//! `marrow test`: check a project, then run its tests over fresh stores.

use marrow_codes::Code;
use std::cell::RefCell;
use std::path::Path;
use std::process::ExitCode;

use marrow_run::{ProjectInvokeError, ProjectMode, ProjectSession, RunOutputSink, SessionEntry};
use marrow_syntax::SourceSpan;
use serde_json::{Value, json};

use crate::cmd_check::located_runtime_fault_line;
use crate::term_style::{self, Stream, Style};
use crate::trace::TraceHook;
use crate::{CheckFormat, report_simple_error, write_json};

const TEST_OUTPUT_CAPTURE_LIMIT: usize = 64 * 1024;
const TEST_OUTPUT_TRUNCATED_MARKER: &str = "\n[output truncated]\n";

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
    let session = match ProjectSession::open(dir, ProjectMode::Test) {
        Ok(session) => session,
        Err(error) => return crate::cmd_run::report_session_open_error(dir, error, format),
    };
    let tests = session.test_cases();
    if tests.is_empty() {
        report_simple_error(
            Code::TestNone.as_str(),
            "no tests found; check the `tests` paths in marrow.json",
            format,
        );
        return ExitCode::FAILURE;
    }
    let selected_tests: Vec<_> = match filter {
        Some(filter) => tests
            .iter()
            .filter(|test| test.name.contains(filter))
            .collect(),
        None => tests.iter().collect(),
    };
    if selected_tests.is_empty() {
        match filter {
            Some(filter) => report_simple_error(
                Code::TestNone.as_str(),
                &format!("no tests matched filter `{filter}`"),
                format,
            ),
            None => report_simple_error(
                Code::TestNone.as_str(),
                "no tests found; check the `tests` paths in marrow.json",
                format,
            ),
        }
        return ExitCode::FAILURE;
    }
    let total = tests.len();
    let selected = selected_tests.len();
    let runtime_program = session.runtime_program();

    // Tests get the same host capabilities as a run; their `std::log` output goes
    // to a discard sink so it stays out of the pass/fail report.
    let nondeterminism = marrow_run::SystemNondeterminism::new();
    let host = crate::cmd_run::base_host(
        std::rc::Rc::new(RefCell::new(DiscardLogSink)),
        &nondeterminism,
    );
    let capture_output = matches!(format, CheckFormat::Json | CheckFormat::Jsonl);
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut errored = 0usize;
    let mut results = Vec::new();
    for test in selected_tests {
        let mut captured_output = TestOutputCapture::new();
        let mut discard_output = |_text: &str| {};
        // Machine-readable reports need bounded scratch output so failing records
        // can include pre-fault prints; passed tests drop this state.
        // A traced test runs under the debugger entry with a hook labelled by the
        // test name, so its statements and writes are attributed to it; an untraced
        // test runs through the plain entry and pays nothing.
        let result = {
            let program_output: &mut dyn RunOutputSink = if capture_output {
                &mut captured_output
            } else {
                &mut discard_output
            };
            if trace {
                let mut hook = TraceHook::new(test.name.clone(), runtime_program);
                let result = session.invoke(
                    SessionEntry::new(&test.name, &host, program_output).with_hook(&mut hook),
                );
                hook.flush();
                result
            } else {
                session.invoke(SessionEntry::new(&test.name, &host, program_output))
            }
        };
        match result {
            Ok(_) => {
                if matches!(format, CheckFormat::Text) {
                    let label = term_style::paint(Stream::Stdout, Style::Success, "ok");
                    println!("{label}    {}", test.name);
                }
                record_test_result(
                    format,
                    &mut results,
                    test_result_record(
                        &test.name,
                        "passed",
                        &test.source_file,
                        test.span,
                        None,
                        None,
                    ),
                );
                passed += 1;
            }
            Err(ProjectInvokeError::Runtime(error)) => {
                // The fault's own origin names the file it was raised in, which
                // differs from the entry's `source_file` for a cross-module fault.
                // The entry file is the fallback when a fault carries no origin.
                let file = error
                    .origin
                    .and_then(|id| runtime_program.file_path(id))
                    .unwrap_or(test.source_file.as_path());
                // An assertion is a test FAIL; any other fault is an ERROR. The
                // labels are column-aligned with the `ok` line.
                let (label, padding, status, counter) = if error.code() == marrow_run::RUN_ASSERT {
                    ("FAIL", "  ", "failed", &mut failed)
                } else {
                    ("ERROR", " ", "errored", &mut errored)
                };
                if matches!(format, CheckFormat::Text) {
                    let label = term_style::paint(Stream::Stdout, Style::Error, label);
                    println!("{label}{padding}{}", test.name);
                    println!(
                        "      {}",
                        located_runtime_fault_line(
                            Stream::Stdout,
                            file,
                            error.span,
                            error.code(),
                            &error.message,
                        )
                    );
                }
                record_test_result(
                    format,
                    &mut results,
                    test_result_record(
                        &test.name,
                        status,
                        file,
                        error.span,
                        Some(FaultDetail {
                            code: error.code(),
                            message: &error.message,
                            data: crate::cmd_check::runtime_fault_data(&error),
                        }),
                        capture_output.then(|| captured_output.report_value()),
                    ),
                );
                *counter += 1;
            }
            Err(ProjectInvokeError::Session(error)) => {
                return crate::cmd_run::report_session_open_error(dir, error, format);
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
            "project": crate::project_json_path(dir),
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

struct DiscardLogSink;

impl marrow_run::LogSink for DiscardLogSink {
    fn write_log(&mut self, _line: &str) {}
}

struct TestOutputCapture {
    text: String,
    truncated: bool,
}

impl TestOutputCapture {
    fn new() -> Self {
        Self {
            text: String::new(),
            truncated: false,
        }
    }

    fn report_value(&self) -> Value {
        if self.text.is_empty() {
            Value::Null
        } else {
            Value::String(self.text.clone())
        }
    }
}

impl RunOutputSink for TestOutputCapture {
    fn write(&mut self, text: &str) {
        if self.truncated || text.is_empty() {
            return;
        }
        if self.text.len() + text.len() <= TEST_OUTPUT_CAPTURE_LIMIT {
            self.text.push_str(text);
            return;
        }

        let content_limit = TEST_OUTPUT_CAPTURE_LIMIT - TEST_OUTPUT_TRUNCATED_MARKER.len();
        if self.text.len() > content_limit {
            let end = char_boundary(&self.text, content_limit);
            self.text.truncate(end);
        }
        let remaining = content_limit.saturating_sub(self.text.len());
        let end = char_boundary(text, remaining);
        self.text.push_str(&text[..end]);
        self.text.push_str(TEST_OUTPUT_TRUNCATED_MARKER);
        self.truncated = true;
    }
}

fn char_boundary(text: &str, limit: usize) -> usize {
    if limit >= text.len() {
        return text.len();
    }
    let mut index = limit;
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// The runtime fault a failed or errored test record reports: the same code, message,
/// and typed `data` payload that `run` and `check` fault records carry, so a machine
/// consumer can report why a test failed without falling back to the text format.
struct FaultDetail<'a> {
    code: &'a str,
    message: &'a str,
    data: serde_json::Map<String, Value>,
}

fn test_result_record(
    name: &str,
    outcome: &str,
    file: &Path,
    span: SourceSpan,
    fault: Option<FaultDetail<'_>>,
    output: Option<Value>,
) -> Value {
    let mut record = serde_json::Map::from_iter([
        ("kind".into(), json!("test")),
        ("name".into(), json!(name)),
        ("outcome".into(), json!(outcome)),
        ("file".into(), json!(file.display().to_string())),
        ("span".into(), span_record(span)),
    ]);
    if let Some(fault) = fault {
        record.insert("code".into(), json!(fault.code));
        record.insert("message".into(), json!(fault.message));
        record.insert("data".into(), Value::Object(fault.data));
    }
    if let Some(output) = output {
        record.insert("output".into(), output);
    }
    Value::Object(record)
}

fn span_record(span: SourceSpan) -> Value {
    json!({
        "line": span.line,
        "column": span.column,
    })
}
