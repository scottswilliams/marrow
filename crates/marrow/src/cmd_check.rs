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
With --locked, a stale or missing marrow.lock is a fatal error for CI rather than an advisory.
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

/// `--locked` over a project that has durable shape to lock — any present store file, whether it
/// opens read-only or is recovery-required after a crash — but no committed `marrow.lock` at all.
/// The gate's contract is that the committed lock is current; an absent lock over durable shape is
/// not current, it is missing, and a CI gate that passed it would give a false green to a developer
/// who forgot to commit or deleted the lock — most dangerously in the post-crash state where the
/// store is present but unreadable. The advisory (non-`--locked`) mode stays silent on absence so a
/// legitimate first run, which has no store and no durable shape to lock yet, checks cleanly.
const CHECK_LOCK_MISSING: &str = "check.lock_missing";

const MISSING_LOCK_MESSAGE: &str =
    "marrow.lock is absent but the store carries durable shape; commit the projected lock";

/// A `marrow check --locked` failure: the project declares a surface and a `client` output, but
/// the committed client is absent or carries a different surface-ABI digest than the current
/// surface. Plain `check` is read-only and surfaces this as a non-fatal advisory; `--locked`
/// makes it fatal so CI fails against a client the surface has outrun. Mirrors the stale-lock
/// family.
const CHECK_STALE_CLIENT: &str = "check.stale_client";

const STALE_CLIENT_MESSAGE: &str = "the declared client is absent or behind the current surface; a run or evolve apply rewrites it";

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
    // Classify the durable store: readable (bind its accepted snapshot), present-but-unreadable
    // (a crashed/locked store that still carries durable shape), or absent (a first run). The
    // read is read-only, so check never opens an unreadable store for repair, creates one, or
    // writes the source tree; only a readable store's snapshot binds the checked program's frozen
    // shapes, while presence alone governs the `--locked` missing-lock gate below.
    let authority = crate::read_accepted_store_catalog_lenient(dir, &config);
    let snapshot = match marrow_check::analyze_project(
        Path::new(dir),
        &config,
        &marrow_check::ProjectSources::new(),
        authority.snapshot(),
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

    // A `--locked` gate over a project that has durable shape to lock but no committed lock at all
    // is a distinct fatal condition: the committed lock is missing, not merely stale. The advisory
    // mode stays silent on absence, so a legitimate first run (no accepted authority yet) is clean.
    if strictness == LockStrictness::Fatal && lock.is_none() && authority.store_present() {
        crate::report_project_failed_with_diagnostic(
            dir,
            &snapshot.report,
            missing_lock_diagnostic(),
            format,
        );
        return ExitCode::FAILURE;
    }

    let stale_lock = lock
        .as_ref()
        .is_some_and(|lock| lock.source_digest != snapshot.program.source_digest());

    // The project declares a `client` output that the surface has outrun: the committed client is
    // absent or carries a different surface-ABI digest than the current surface. The shared
    // freshness owner returns `Rewritten` for exactly that stale-or-absent state; a project with no
    // `client` (`NotConfigured`) or no surface to project (`SurfacelessConfigured`), or an
    // already-current client (`AlreadyFresh`), raises no client condition. Check is read-only and
    // never writes; it only reads the verdict the write path would act on.
    let stale_client = matches!(
        crate::client_freshness(dir, &config, &snapshot.program),
        crate::ClientFreshness::Rewritten
    );

    // A fatal lock or client condition is a check failure, so its report must mirror any other
    // failing check: the structured envelope reports `failed`, carries the condition's diagnostic,
    // and omits the success-only entry footprints and surface descriptors. The lock gate is checked
    // first so its diagnostic leads when both fault under `--locked`.
    if strictness == LockStrictness::Fatal {
        if stale_lock {
            crate::report_project_failed_with_diagnostic(
                dir,
                &snapshot.report,
                stale_lock_diagnostic(),
                format,
            );
            return ExitCode::FAILURE;
        }
        if stale_client {
            crate::report_project_failed_with_diagnostic(
                dir,
                &snapshot.report,
                stale_client_diagnostic(),
                format,
            );
            return ExitCode::FAILURE;
        }
    }

    // The advisory (non-`--locked`) case keeps the clean success envelope and surfaces each stale
    // condition as a separate stderr note. Both advisories travel through one success render so a
    // project that is both lock-advisory and client-advisory still reports a single `ok` envelope.
    let mut advisories = Vec::new();
    if stale_lock {
        advisories.push(crate::ProjectAdvisory {
            diagnostic: stale_lock_advisory_diagnostic(),
            note: format!("{CHECK_STALE_LOCK}: {STALE_LOCK_MESSAGE}"),
            summary: format!("marrow.lock is stale - run marrow run {dir} to refresh"),
        });
    }
    if stale_client {
        advisories.push(crate::ProjectAdvisory {
            diagnostic: stale_client_advisory_diagnostic(),
            note: format!("{CHECK_STALE_CLIENT}: {STALE_CLIENT_MESSAGE}"),
            summary: format!("the declared client is stale - run marrow run {dir} to refresh"),
        });
    }

    if advisories.is_empty() {
        crate::report_project_with_program(dir, &snapshot.report, &snapshot.program, format);
    } else {
        crate::report_project_ok_with_advisories(
            dir,
            &snapshot.report,
            &snapshot.program,
            advisories,
            format,
        );
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

/// The missing-lock condition as a structured diagnostic for the failed-check envelope. Like the
/// stale-lock diagnostic it carries no source span: it reports that the whole committed projection
/// is absent rather than faulting at a single declaration.
fn missing_lock_diagnostic() -> serde_json::Value {
    serde_json::json!({
        "code": CHECK_LOCK_MISSING,
        "kind": marrow_syntax::kind_for_code(CHECK_LOCK_MISSING),
        "message": MISSING_LOCK_MESSAGE,
        "severity": "error",
        "source_span": null,
    })
}

/// The non-fatal stale-lock advisory as a structured diagnostic for the success envelope. It is
/// a warning, not an error, so a clean check still passes; carrying it in the stdout envelope
/// lets a machine consumer see the advisory without parsing stderr. A later `run` or
/// `evolve apply` re-projects the lock to converge it.
fn stale_lock_advisory_diagnostic() -> serde_json::Value {
    serde_json::json!({
        "code": CHECK_STALE_LOCK,
        "kind": marrow_syntax::kind_for_code(CHECK_STALE_LOCK),
        "message": STALE_LOCK_MESSAGE,
        "severity": "warning",
        "source_span": null,
    })
}

/// The stale-client condition as a structured diagnostic for the failed-check envelope. Like the
/// lock diagnostics it carries no source span: it compares the committed client against the whole
/// projected surface rather than faulting at a single declaration.
fn stale_client_diagnostic() -> serde_json::Value {
    serde_json::json!({
        "code": CHECK_STALE_CLIENT,
        "kind": marrow_syntax::kind_for_code(CHECK_STALE_CLIENT),
        "message": STALE_CLIENT_MESSAGE,
        "severity": "error",
        "source_span": null,
    })
}

/// The non-fatal stale-client advisory as a structured diagnostic for the success envelope. It is
/// a warning, not an error, so a clean check still passes; carrying it in the stdout envelope lets
/// a machine consumer see the advisory without parsing stderr. A later `run` or `evolve apply`
/// rewrites the client to converge it.
fn stale_client_advisory_diagnostic() -> serde_json::Value {
    serde_json::json!({
        "code": CHECK_STALE_CLIENT,
        "kind": marrow_syntax::kind_for_code(CHECK_STALE_CLIENT),
        "message": STALE_CLIENT_MESSAGE,
        "severity": "warning",
        "source_span": null,
    })
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
