//! `marrow test`: check a project, then run its tests over fresh stores.

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::trace::TraceHook;
use crate::{CheckFormat, load_checked_project, report_project, report_simple_error};

/// Run a project's tests: `marrow test [--trace] <projectdir>`.
pub(crate) fn test(args: &[String]) -> ExitCode {
    let mut dir = None;
    let mut trace = false;
    let mut format = CheckFormat::Text;
    let mut saw_format = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            // Report each statement and managed write of every test as it runs,
            // attributed to the test by name.
            "--trace" => trace = true,
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
                    eprintln!("unknown format: {value}");
                    return ExitCode::from(2);
                };
                format = parsed;
            }
            "--help" | "-h" => {
                print!(
                    "\
Usage:
  marrow test [--trace] [--format text|json|jsonl] <projectdir>

Check a Marrow project, then run its tests: every `pub fn` with no parameters in
a test file (the `tests` patterns in marrow.json). Each test runs against a fresh
in-memory store; a `std::assert::*` failure is a located test failure.

  --trace   Report each statement and managed write of every test as it runs,
            attributed to the test by name. Takes --format for the trace output.
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
    test_project_dir(&dir, trace, format)
}

/// Check `<dir>`'s project and its test files, then run each test over a fresh
/// in-memory store. Reports each result and a summary; exits non-zero if any test
/// fails or errors, if the project does not check, or if no tests are found. With
/// `trace`, each test runs under an execution trace attributed to it by name.
fn test_project_dir(dir: &str, trace: bool, format: CheckFormat) -> ExitCode {
    let (config, src_program) = match load_checked_project(dir) {
        Ok(checked) => checked,
        Err(code) => return code,
    };
    let source_module_count = src_program.modules.len();

    let (test_report, program) =
        match marrow_check::check_tests_program(std::path::Path::new(dir), &config, &src_program) {
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
    let tests: Vec<(String, PathBuf)> = program.modules[source_module_count..]
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
        // A traced test runs under the debugger entry with a hook labelled by the
        // test name, so its statements and writes are attributed to it; an untraced
        // test runs through the plain entry and pays nothing.
        let result = if trace {
            let mut hook = TraceHook::new(format, name.clone());
            let result =
                marrow_run::run_entry_with_debugger(&program, &store, &host, &mut hook, name, &[]);
            hook.flush();
            result
        } else {
            marrow_run::run_entry_with_host(&program, &store, &host, name, &[])
        };
        match result {
            Ok(_) => {
                println!("ok    {name}");
                passed += 1;
            }
            Err(error) => {
                // The fault's own origin names the file it was raised in, which
                // differs from the entry's `source_file` for a cross-module fault.
                // The entry file is the fallback when a fault carries no origin.
                let file = error
                    .origin
                    .and_then(|id| program.file_path(id))
                    .unwrap_or(source_file.as_path());
                // An assertion is a test FAIL; any other fault is an ERROR. The
                // labels are column-aligned with the `ok` line.
                let (label, counter) = if error.code == marrow_run::RUN_ASSERT {
                    ("FAIL ", &mut failed)
                } else {
                    ("ERROR", &mut errored)
                };
                println!("{label} {name}");
                println!(
                    "      {}:{}:{}: {}: {}",
                    file.display(),
                    error.span.line,
                    error.span.column,
                    error.code,
                    error.message
                );
                *counter += 1;
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
