//! `marrow test`: check a project, then run its tests over fresh stores.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use marrow_syntax::SourceSpan;
use serde_json::{Value, json};

use crate::trace::TraceHook;
use crate::{
    CheckFormat, load_checked_project_with_format, report_project, report_simple_error, write_json,
};

/// Run a project's tests: `marrow test [--trace] [--format text|json|jsonl] [--filter <substring>] <projectdir>`.
pub(crate) fn test(args: &[String]) -> ExitCode {
    let mut dir = None;
    let mut trace = false;
    let mut format = CheckFormat::Text;
    let mut saw_format = false;
    let mut filter = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--trace" => trace = true,
            "--filter" => {
                if filter.is_some() {
                    eprintln!("duplicate --filter");
                    return ExitCode::from(2);
                }
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --filter");
                    return ExitCode::from(2);
                };
                filter = Some(value.clone());
            }
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
  marrow test [--trace] [--format text|json|jsonl] [--filter <substring>] <projectdir>

Check a Marrow project, then run its tests: every `pub fn` with no parameters in
a test file (the `tests` paths in marrow.json). Each test runs against a fresh
in-memory store; a `std::assert::*` failure is a located test failure.

  --filter  Run only tests whose qualified name contains the substring.
  --format  Shape the test report on stdout.
  --trace   Report each statement and managed write of every test as a text
            stderr trace stream attributed by test name.
"
                );
                return ExitCode::SUCCESS;
            }
            value if value.starts_with('-') => return crate::unknown_option("test", value),
            value => {
                if let Err(code) =
                    crate::take_single_target(&mut dir, value, "test", "project directory")
                {
                    return code;
                }
            }
        }
        index += 1;
    }

    let Some(dir) = dir else {
        eprintln!("missing project directory");
        return ExitCode::from(2);
    };
    if trace && !matches!(format, CheckFormat::Text) {
        eprintln!("--trace only supports --format text");
        return ExitCode::from(2);
    }
    test_project_dir(&dir, trace, format, filter.as_deref())
}

/// Check `<dir>`'s project and its test files, then run each test over a fresh
/// in-memory store. Reports each result and a summary; exits non-zero if any test
/// fails or errors, if the project does not check, or if no tests are found. With
/// `trace`, each test runs under an execution trace attributed to it by name.
fn test_project_dir(dir: &str, trace: bool, format: CheckFormat, filter: Option<&str>) -> ExitCode {
    let (config, src_program) = match load_checked_project_with_format(dir, format) {
        Ok(checked) => checked,
        Err(code) => return code,
    };
    // Tests run over throwaway in-memory stores, never the project's durable store, so a
    // project that has not yet committed its baseline binds its proposed identity here: the
    // saved-root catalog ids the test writes address resolve against the identity a first
    // run would freeze, without touching any durable store.
    let src_program = match bind_test_identity(dir, &config, src_program, format) {
        Ok(program) => program,
        Err(code) => return code,
    };
    let source_module_count = src_program.modules.len();

    let (test_report, program) =
        match marrow_check::check_tests_program(std::path::Path::new(dir), &config, src_program) {
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
    if test_report.has_errors() {
        report_project(dir, &test_report, format);
        return ExitCode::FAILURE;
    }

    // Each test keeps its source file so a failure can be reported at its location.
    let tests: Vec<TestCase> = program.modules[source_module_count..]
        .iter()
        .flat_map(|module| {
            module
                .functions
                .iter()
                .filter(|function| function.public && function.params.is_empty())
                .map(|function| TestCase {
                    name: format!("{}::{}", module.name, function.name),
                    source_file: module.source_file.clone(),
                    span: function.span,
                })
        })
        .collect();
    if tests.is_empty() {
        report_simple_error(
            "test.none",
            "no tests found; check the `tests` paths in marrow.json",
            format,
        );
        return ExitCode::FAILURE;
    }
    let selected_tests: Vec<&TestCase> = match filter {
        Some(filter) => tests
            .iter()
            .filter(|test| test.name.contains(filter))
            .collect(),
        None => tests.iter().collect(),
    };
    if selected_tests.is_empty() {
        let filter = filter.expect("selected tests are empty only after filtering");
        report_simple_error(
            "test.none",
            &format!("no tests matched filter `{filter}`"),
            format,
        );
        return ExitCode::FAILURE;
    }
    let total = tests.len();
    let selected = selected_tests.len();
    let runtime_program = program.runtime();

    // Tests get the same host capabilities as a run; their `std::log` output goes
    // to a discard sink so it stays out of the pass/fail report.
    let nondeterminism = marrow_run::SystemNondeterminism::new();
    let host = crate::cmd_run::base_host(
        std::rc::Rc::new(RefCell::new(String::new())),
        &nondeterminism,
    );
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut errored = 0usize;
    let mut results = Vec::new();
    for test in selected_tests {
        let store = marrow_store::tree::TreeStore::memory();
        let mut program_output = |_text: &str| {};
        // A traced test runs under the debugger entry with a hook labelled by the
        // test name, so its statements and writes are attributed to it; an untraced
        // test runs through the plain entry and pays nothing.
        let result =
            match marrow_run::CheckedEntryCall::new(&runtime_program, &test.name, Vec::new()) {
                Err(error) => Err(error),
                Ok(call) if trace => {
                    let mut hook = TraceHook::new(test.name.clone(), &runtime_program);
                    let result = marrow_run::run_entry_with_debugger(
                        &store,
                        &host,
                        &mut hook,
                        &call,
                        &mut program_output,
                    );
                    hook.flush();
                    result
                }
                Ok(call) => {
                    marrow_run::run_entry_with_host(&store, &host, &call, &mut program_output)
                }
            };
        match result {
            Ok(_) => {
                if matches!(format, CheckFormat::Text) {
                    println!("ok    {}", test.name);
                }
                record_test_result(
                    format,
                    &mut results,
                    test_result_record(&test.name, "passed", &test.source_file, test.span, None),
                );
                passed += 1;
            }
            Err(error) => {
                // The fault's own origin names the file it was raised in, which
                // differs from the entry's `source_file` for a cross-module fault.
                // The entry file is the fallback when a fault carries no origin.
                let file = error
                    .origin
                    .and_then(|id| runtime_program.file_path(id))
                    .unwrap_or(test.source_file.as_path());
                // An assertion is a test FAIL; any other fault is an ERROR. The
                // labels are column-aligned with the `ok` line.
                let (label, status, counter) = if error.code == marrow_run::RUN_ASSERT {
                    ("FAIL ", "failed", &mut failed)
                } else {
                    ("ERROR", "errored", &mut errored)
                };
                if matches!(format, CheckFormat::Text) {
                    println!("{label} {}", test.name);
                    println!(
                        "      {}:{}:{}: {}: {}",
                        file.display(),
                        error.span.line,
                        error.span.column,
                        error.code,
                        error.message
                    );
                }
                record_test_result(
                    format,
                    &mut results,
                    test_result_record(&test.name, status, file, error.span, Some(error.code)),
                );
                *counter += 1;
            }
        }
    }
    let summary = TestSummary {
        total,
        selected,
        passed,
        failed,
        errored,
    };
    report_test_results(dir, format, &results, summary);
    if failed == 0 && errored == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

struct TestCase {
    name: String,
    source_file: PathBuf,
    span: SourceSpan,
}

#[derive(Clone, Copy)]
struct TestSummary {
    total: usize,
    selected: usize,
    passed: usize,
    failed: usize,
    errored: usize,
}

fn report_test_results(dir: &str, format: CheckFormat, results: &[Value], summary: TestSummary) {
    match format {
        CheckFormat::Text if summary.selected == summary.total => {
            println!(
                "\n{} test{}: {} passed, {} failed, {} errored",
                summary.selected,
                if summary.selected == 1 { "" } else { "s" },
                summary.passed,
                summary.failed,
                summary.errored
            );
        }
        CheckFormat::Text => {
            println!(
                "\n{} of {} selected tests: {} passed, {} failed, {} errored",
                summary.selected, summary.total, summary.passed, summary.failed, summary.errored
            );
        }
        CheckFormat::Json => write_json(json!({
            "project": dir,
            "tests": results,
            "summary": test_summary_record(summary),
        })),
        CheckFormat::Jsonl => write_json(json!({
            "kind": "summary",
            "total": summary.total,
            "selected": summary.selected,
            "passed": summary.passed,
            "failed": summary.failed,
            "errored": summary.errored,
        })),
    }
}

fn record_test_result(format: CheckFormat, results: &mut Vec<Value>, result: Value) {
    if matches!(format, CheckFormat::Jsonl) {
        write_json(result.clone());
    }
    results.push(result);
}

fn test_summary_record(summary: TestSummary) -> Value {
    json!({
        "total": summary.total,
        "selected": summary.selected,
        "passed": summary.passed,
        "failed": summary.failed,
        "errored": summary.errored,
    })
}

fn test_result_record(
    name: &str,
    outcome: &str,
    file: &Path,
    span: SourceSpan,
    code: Option<&str>,
) -> Value {
    let mut record = serde_json::Map::from_iter([
        ("kind".into(), json!("test")),
        ("name".into(), json!(name)),
        ("outcome".into(), json!(outcome)),
        ("file".into(), json!(file.display().to_string())),
        ("span".into(), span_record(span)),
    ]);
    if let Some(code) = code {
        record.insert("code".into(), json!(code));
    }
    Value::Object(record)
}

fn span_record(span: SourceSpan) -> Value {
    json!({
        "line": span.line,
        "column": span.column,
    })
}

/// Bind a project's proposed durable identity for a test run. A project that has not yet
/// committed its baseline (no accepted catalog, a non-empty proposal) is re-checked with
/// the proposal as the accepted catalog, so its saved-root catalog ids resolve when a test
/// writes saved data. A project past its baseline already binds its accepted identity and
/// is returned unchanged. The store the proposal would freeze is never touched: tests run
/// over throwaway in-memory stores.
fn bind_test_identity(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    program: marrow_check::CheckedProgram,
    format: CheckFormat,
) -> Result<marrow_check::CheckedProgram, ExitCode> {
    if program.catalog.accepted_epoch.is_some() {
        return Ok(program);
    }
    let Some(proposal) = program.catalog.proposal.clone() else {
        return Ok(program);
    };
    if proposal.entries.is_empty() {
        return Ok(program);
    }
    let (report, bound) = marrow_check::check_project_with_catalog(
        std::path::Path::new(dir),
        config,
        Some(&proposal),
    )
    .map_err(|error| {
        report_simple_error(
            error.code,
            &format!("{}: {}", error.path.display(), error.message),
            format,
        );
        ExitCode::FAILURE
    })?;
    if report.has_errors() {
        report_project(dir, &report, format);
        return Err(ExitCode::FAILURE);
    }
    Ok(bound)
}
