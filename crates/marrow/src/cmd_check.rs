//! `marrow check`: type-check a project, and the shared
//! located-fault renderer the runner reuses.

use std::path::Path;
use std::process::ExitCode;

use crate::{CheckFormat, report_simple_error, write_json_err};

pub(crate) fn check(args: &[String]) -> ExitCode {
    let mut format = CheckFormat::Text;
    let mut saw_format = false;
    let mut target = None;
    let mut locked = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--format" => {
                if let Err(code) =
                    crate::parse_format_flag(args, &mut index, &mut saw_format, &mut format)
                {
                    return code;
                }
            }
            "--locked" => {
                if locked {
                    eprintln!("duplicate --locked");
                    return ExitCode::from(2);
                }
                locked = true;
            }
            "--help" | "-h" => {
                print!(
                    "\
Usage:
  marrow check [--format text|json|jsonl] [--locked] <projectdir>

Check a project directory containing marrow.json and report diagnostics.
With --locked, a stale marrow.lock is a fatal error for CI rather than an advisory.
"
                );
                return ExitCode::SUCCESS;
            }
            value if value.starts_with('-') => return crate::unknown_option("check", value),
            value => {
                if let Err(code) =
                    crate::take_single_target(&mut target, value, "check", "project directory")
                {
                    return code;
                }
            }
        }
        index += 1;
    }

    let Some(target) = target else {
        eprintln!("missing project directory");
        return ExitCode::from(2);
    };
    if let Err(code) = crate::reject_bare_file_target("check", &target) {
        return code;
    }
    check_project_dir(&target, format, locked)
}

/// How a stale committed lock is treated. The ordinary edit -> check -> run loop edits source ahead
/// of the next write path, so a stale lock is a non-fatal advisory by default; `--locked` (the
/// lockfile-ecosystem convention) makes it fatal so CI fails against a lock the source has outrun.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LockStrictness {
    Advisory,
    Fatal,
}

/// A committed `marrow.lock` whose recorded source digest no longer matches the digest of the
/// current checked source. `marrow check` is read-only: it cannot re-project the lock, so it
/// surfaces staleness as a non-fatal advisory rather than failing. Editing source ahead of the
/// next write path is the ordinary case, so a stale lock does not block a clean check; a later
/// `run` or `evolve apply` re-projects the lock to converge it.
const CHECK_STALE_LOCK: &str = "check.stale_lock";

/// Check a whole project: load `<dir>/marrow.json`, then run the project
/// checker over its source roots and configured test files. The committed lock binds first-run
/// identity and, when it disagrees with the current source digest, surfaces a stale-lock condition
/// — a non-fatal advisory by default, fatal under `--locked`; check never opens or repairs the
/// store.
fn check_project_dir(dir: &str, format: CheckFormat, locked: bool) -> ExitCode {
    let strictness = if locked {
        LockStrictness::Fatal
    } else {
        LockStrictness::Advisory
    };
    let config = match crate::load_config_with_format(dir, format) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let lock = match crate::read_committed_lock(dir, format) {
        Ok(lock) => lock,
        Err(code) => return code,
    };
    // Bind from the store when one is present and readable, falling back to the committed lock
    // otherwise. The store is the accepted authority — binding its snapshot gives the checked
    // program its frozen shapes — and the read is read-only, so check never opens an unreadable
    // store for repair, creates one, or writes the source tree. A store that cannot be opened
    // read-only is treated as no accepted authority, leaving the lock as the first-run anchor.
    let accepted = crate::read_accepted_store_catalog_lenient(dir, &config);
    let snapshot = match marrow_check::analyze_project(
        Path::new(dir),
        &config,
        &marrow_check::ProjectSources::new(),
        accepted.as_ref(),
        lock.as_ref(),
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            report_simple_error(
                error.code,
                &format!("{}: {}", error.path.display(), error.message),
                format,
            );
            return ExitCode::FAILURE;
        }
    };

    if snapshot.report.has_errors() {
        crate::report_project(dir, &snapshot.report, format);
        return ExitCode::FAILURE;
    }
    let stale = lock
        .as_ref()
        .is_some_and(|lock| lock.source_digest != snapshot.program.source_digest());

    // A fatal stale lock is a check failure, so its report must mirror any other failing check:
    // the structured envelope reports `failed`, carries the stale-lock diagnostic, and omits the
    // success-only entry footprints and surface descriptors. The advisory case keeps the clean
    // success envelope and surfaces the stale lock as a separate stderr note.
    if stale && strictness == LockStrictness::Fatal {
        crate::report_project_failed_with_diagnostic(
            dir,
            &snapshot.report,
            stale_lock_diagnostic(),
            format,
        );
        return ExitCode::FAILURE;
    }

    crate::report_project_with_program(dir, &snapshot.report, &snapshot.program, format);
    if stale {
        report_stale_lock_advisory(format);
    }
    ExitCode::SUCCESS
}

const STALE_LOCK_MESSAGE: &str =
    "marrow.lock is behind the current source; a run or evolve apply re-projects it";

/// The stale-lock condition as a structured diagnostic for the failed-check envelope. It carries
/// no source span: it compares the committed lock against the whole checked source rather than
/// faulting at a single declaration.
fn stale_lock_diagnostic() -> serde_json::Value {
    serde_json::json!({
        "code": CHECK_STALE_LOCK,
        "kind": marrow_syntax::kind_for_code(CHECK_STALE_LOCK),
        "message": STALE_LOCK_MESSAGE,
        "severity": "error",
        "source_span": null,
    })
}

/// Note the non-fatal stale-lock advisory on stderr, off the success envelope on stdout, so the
/// structured result stays a single parseable value. A later `run` or `evolve apply` re-projects
/// the lock to converge it.
fn report_stale_lock_advisory(format: CheckFormat) {
    match format {
        CheckFormat::Text => eprintln!("{CHECK_STALE_LOCK}: {STALE_LOCK_MESSAGE}"),
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json_err(serde_json::json!({
                "code": CHECK_STALE_LOCK,
                "kind": marrow_syntax::kind_for_code(CHECK_STALE_LOCK),
                "message": STALE_LOCK_MESSAGE,
                "data": {},
                "source_span": null,
            }));
        }
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
    format: CheckFormat,
) {
    match format {
        CheckFormat::Text => match error.origin.and_then(|id| program.file_path(id)) {
            Some(path) => eprintln!(
                "{}:{}:{}: {}: {}",
                path.display(),
                error.span.line,
                error.span.column,
                error.code(),
                error.message
            ),
            None => report_simple_error(error.code(), &error.message, CheckFormat::Text),
        },
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json_err(serde_json::Value::Object(runtime_fault_json(
                program, error,
            )));
        }
    }
}

pub(crate) fn runtime_fault_json(
    program: &marrow_check::CheckedRuntimeProgram,
    error: &marrow_run::RuntimeError,
) -> serde_json::Map<String, serde_json::Value> {
    let path = error.origin.and_then(|id| program.file_path(id));
    let mut data = serde_json::Map::new();
    if let Some(code) = error.uncaught_throw_code() {
        data.insert("code".to_string(), serde_json::json!(code));
    }
    if let Some(call_depth) = error.call_depth() {
        data.insert(
            "callee".to_string(),
            serde_json::json!(call_depth.function_name),
        );
        data.insert("budget".to_string(), serde_json::json!(call_depth.budget));
        data.insert(
            "observed_depth".to_string(),
            serde_json::json!(call_depth.observed_depth),
        );
    }
    let source_span = match path {
        Some(path) => serde_json::json!({
            "file": path.display().to_string(),
            "line": error.span.line,
            "column": error.span.column,
        }),
        None => serde_json::Value::Null,
    };
    serde_json::Map::from_iter([
        ("code".to_string(), serde_json::json!(error.code())),
        (
            "kind".to_string(),
            serde_json::json!(marrow_syntax::kind_for_code(error.code())),
        ),
        ("message".to_string(), serde_json::json!(error.message)),
        ("data".to_string(), serde_json::Value::Object(data)),
        ("source_span".to_string(), source_span),
    ])
}
