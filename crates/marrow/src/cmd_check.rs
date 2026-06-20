//! `marrow check`: type-check a project, and the shared
//! located-fault renderer the runner reuses.

use std::path::Path;
use std::process::ExitCode;

use crate::{CheckFormat, report_simple_error, write_json_err};

pub(crate) fn check(args: &[String]) -> ExitCode {
    let mut format = CheckFormat::Text;
    let mut saw_format = false;
    let mut target = None;
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
            "--help" | "-h" => {
                print!(
                    "\
Usage:
  marrow check [--format text|json|jsonl] <projectdir>

Check a project directory containing marrow.json and report diagnostics.
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
    check_project_dir(&target, format)
}

/// Check a whole project: load `<dir>/marrow.json`, then run the project
/// checker over its source roots and configured test files.
fn check_project_dir(dir: &str, format: CheckFormat) -> ExitCode {
    let config = match crate::load_config_with_format(dir, format) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let accepted = match crate::read_accepted_catalog_artifact(dir, format) {
        Ok(accepted) => accepted,
        Err(code) => return code,
    };
    let snapshot = match marrow_check::analyze_project(
        Path::new(dir),
        &config,
        &marrow_check::ProjectSources::new(),
        accepted.as_ref(),
        None,
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
        ExitCode::FAILURE
    } else {
        crate::report_project_with_program(dir, &snapshot.report, &snapshot.program, format);
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
