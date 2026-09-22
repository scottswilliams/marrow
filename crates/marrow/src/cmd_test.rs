//! `marrow test [--format text|jsonl] [--filter <substring>]`.
//!
//! Discover `test "name"` declarations from the captured project, compile them
//! into a separately verified image carrying the closed TEST-ENTRY table, prepare
//! that image once, and run each entry through the VM as a fresh test the lifecycle
//! selects from the prepared image. A storeless test (empty reconstructed demand)
//! runs with no session and no store; a durable test runs against its own fresh
//! in-memory store bounded by the test-image demand union, so tests never observe
//! one another's writes. A passing test reports `passed`; a false `assert` or an `err` at a
//! `try` (`run.assert`) reports `failed`; any other runtime fault reports `errored`. Output is a typed
//! `kind: "test"` JSONL stream ending in a summary, or human text. The command
//! exits nonzero when any test fails or errors.

use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use marrow_codes::Code;
use marrow_compile::compile_with_tests;
use marrow_project::ProjectInput;

use crate::Command;
use crate::cmd_run::compile_or_mint;
use crate::command_output::{Flags, OutputFormat, unknown_option, usage};
use crate::outcome::{Record, TestOutcome, TestRecord, TestSummary};
use crate::project::capture_project;
use crate::term_style::{Palette, Stream};

pub(crate) const HELP: &str = "\
Usage:
  marrow test [--format text|jsonl] [--filter <substring>]

Run every `test` declaration in the project at the working directory and report each
outcome: `passed`, `failed` for a false `assert` or an `err` reaching a `try`,
`errored` for any other runtime fault, or `incomplete` for a durable fault that interrupts a commit. A test that
touches no durable place runs with no store; one that does runs against its own
fresh in-memory store. Missing durable identities are minted into `.marrow/ids`
first, as `marrow run` mints them. --filter selects the tests whose title contains the substring
and refuses a substring no test matches. The command exits 0 when every selected
test passes, 1 when any fails or errors, and 2 on a usage error.
";

struct TestArgs {
    format: OutputFormat,
    filter: Option<String>,
}

pub(crate) fn test(rest: &[String]) -> ExitCode {
    let args = match parse_args(rest) {
        Ok(args) => args,
        Err(code) => return code,
    };

    // A live `.marrow/ids` publication marker makes the committed ledger
    // indeterminate, so a command that may mint settles it before capture.
    if let Err(failure) = crate::project::recover_identity_publication(Path::new(".")) {
        return emit_records(args.format, &[Record::capture(failure)], ExitCode::FAILURE);
    }
    let project = match capture_project(Path::new(".")) {
        Ok(project) => project,
        Err(failure) => {
            return emit_records(args.format, &[Record::capture(failure)], ExitCode::FAILURE);
        }
    };
    // Family 1: source diagnostics, including a malformed test and an `assert` outside
    // a test. Missing identities are minted as storeless `marrow run` mints them.
    let (project, compiled) = match compile_or_mint(project, compile_with_tests) {
        Ok(compiled) => compiled,
        Err(records) => return emit_records(args.format, &records, ExitCode::FAILURE),
    };

    // Family 2: artifact decode/verify rejection. The verifier independently
    // rechecks the TEST-ENTRY table and that `assert` sits only in a test entry.
    let image = match marrow_verify::verify(&compiled.image.bytes) {
        Ok(image) => image,
        Err(rejection) => {
            return emit_records(
                args.format,
                &[Record::ArtifactRejected {
                    code: rejection.code(),
                }],
                ExitCode::FAILURE,
            );
        }
    };

    let prepared = marrow_vm::prepare(image);
    let total = prepared.image().test_entries().len();
    let mut records: Vec<TestRecord> = Vec::new();
    let (mut passed, mut failed, mut errored) = (0usize, 0usize, 0usize);

    for (index, entry) in prepared.image().test_entries().iter().enumerate() {
        if let Some(filter) = &args.filter
            && !entry.name().contains(filter.as_str())
        {
            continue;
        }
        // The compiler and the verified image agree on the test set, so the report
        // metadata (file and declaration position) is always found.
        let meta = compiled
            .tests
            .iter()
            .find(|test| test.name == entry.name())
            .expect("compiler and image agree on the test set");

        // Family 3: a source-mapped runtime fault, or a pass. Each call is an
        // invocation boundary: a mutating export commits, and a later reader observes
        // it. A durable shape the ephemeral kernel does not yet execute is reported as
        // the trough.
        let test = marrow_vm::fresh_test(&prepared, index)
            .expect("the entry index came from the prepared image's own test table");
        let outcome = durable_outcome(marrow_vm::run_test(test), meta, &project);
        match &outcome {
            TestOutcome::Passed => passed += 1,
            TestOutcome::Failed { .. } => failed += 1,
            TestOutcome::Errored { .. } | TestOutcome::Incomplete { .. } => errored += 1,
        }
        records.push(TestRecord {
            name: entry.name().to_string(),
            file: meta.file.clone(),
            decl_line: meta.line,
            decl_column: meta.column,
            outcome,
        });
    }

    // A `--filter` that selects nothing is a usage failure, so a mistyped filter is
    // not silently reported as an all-clear.
    if args.filter.is_some() && records.is_empty() {
        return usage(Command::Test, "no test matches the filter");
    }

    let summary = TestSummary {
        passed,
        failed,
        errored,
        total,
    };
    let exit = if failed > 0 || errored > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    };
    emit_tests(args.format, &records, &summary, exit)
}

/// Map a durable VM run into a test outcome. A run classifies by its result; a
/// durable shape the ephemeral kernel does not yet execute, or an operational mint
/// failure, reports at the test's declaration position.
fn durable_outcome(
    run: marrow_vm::DurableRun,
    meta: &marrow_compile::TestEntry,
    project: &ProjectInput,
) -> TestOutcome {
    match run {
        marrow_vm::DurableRun::Ran(result) => classify(result, meta, project),
        marrow_vm::DurableRun::Parked => TestOutcome::Errored {
            code: Code::CliDurableUnsupported,
            line: meta.line,
            column: meta.column,
        },
        marrow_vm::DurableRun::Failed(code) => TestOutcome::Errored {
            code,
            line: meta.line,
            column: meta.column,
        },
    }
}

/// Classify a VM run result into a test outcome: a value or unit return passes, a
/// false `assert` (`run.assert`) fails and carries the assertion's own source line, and
/// any other source-mapped runtime fault errors.
fn classify(
    result: Result<Option<marrow_vm::Value>, marrow_vm::DurableExecutionFault>,
    meta: &marrow_compile::TestEntry,
    project: &ProjectInput,
) -> TestOutcome {
    match result {
        Ok(_) => TestOutcome::Passed,
        Err(marrow_vm::DurableExecutionFault::Runtime(fault))
            if fault.code() == Code::RunAssert =>
        {
            TestOutcome::Failed {
                code: fault.code(),
                line: fault.line(),
                column: fault.column(),
                source_line: source_line(project, meta, fault.line()),
            }
        }
        Err(marrow_vm::DurableExecutionFault::Runtime(fault)) => TestOutcome::Errored {
            code: fault.code(),
            line: fault.line(),
            column: fault.column(),
        },
        Err(marrow_vm::DurableExecutionFault::Incomplete(incomplete)) => {
            let (fault, durable) = match incomplete.into_disposition() {
                marrow_vm::IncompleteDisposition::Classified { fault, durable } => (fault, durable),
                marrow_vm::IncompleteDisposition::Pending { fault, recovery } => {
                    // A source test owns one disposable in-memory attachment. Its engine
                    // cannot report an indeterminate commit; if that changes, discarding the
                    // test attachment is the required retirement before reporting Unknown.
                    drop(recovery);
                    (fault, marrow_vm::DurableCommitState::Unknown)
                }
            };
            TestOutcome::Incomplete {
                code: fault.code(),
                durable,
                line: fault.line(),
                column: fault.column(),
            }
        }
    }
}

/// The trimmed source line a failed assertion sits on: the test's module, at the
/// position the compiler recorded for it in the captured project's module order, read
/// at the fault's line. The image the VM ran was compiled from this capture, so the
/// line is inside the source; the rendering is total either way.
fn source_line(project: &ProjectInput, meta: &marrow_compile::TestEntry, line: u32) -> String {
    let module = &project.modules()[meta.module_index];
    std::str::from_utf8(module.source())
        .ok()
        .zip((line as usize).checked_sub(1))
        .and_then(|(source, index)| source.lines().nth(index))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn parse_args(rest: &[String]) -> Result<TestArgs, ExitCode> {
    const COMMAND: Command = Command::Test;
    let mut format: Option<OutputFormat> = None;
    let mut filter: Option<String> = None;
    let mut flags = Flags::new(rest, COMMAND);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--format" => flags.read_format(&mut format)?,
            "--filter" => flags.read(&mut filter, "--filter", str::to_string)?,
            other => return Err(unknown_option(COMMAND, other)),
        }
    }
    Ok(TestArgs {
        format: format.unwrap_or(OutputFormat::Text),
        filter,
    })
}

/// Emit typed failure records (capture/compile/verify) and return `exit`.
fn emit_records(format: OutputFormat, records: &[Record], exit: ExitCode) -> ExitCode {
    crate::command_output::finish(emit_records_to(
        &mut io::stdout().lock(),
        Palette::for_stream(Stream::Stdout),
        format,
        records,
        exit,
    ))
}

fn emit_records_to(
    writer: &mut impl Write,
    palette: Palette,
    format: OutputFormat,
    records: &[Record],
    exit: ExitCode,
) -> io::Result<ExitCode> {
    // The test command's typed failure records are never a value, so they carry no
    // record types to render.
    for record in records {
        match format {
            OutputFormat::Jsonl => writeln!(
                writer,
                "{}",
                record.to_jsonl(&[], &[]).expect("non-value failure record")
            )?,
            OutputFormat::Text => {
                let text = record
                    .to_text(palette, &[], &[])
                    .expect("non-value failure record");
                if !text.is_empty() {
                    writeln!(writer, "{text}")?;
                }
            }
        }
    }
    writer.flush()?;
    Ok(exit)
}

/// Emit each test record then the summary in the selected format, returning `exit`.
fn emit_tests(
    format: OutputFormat,
    records: &[TestRecord],
    summary: &TestSummary,
    exit: ExitCode,
) -> ExitCode {
    crate::command_output::finish(emit_tests_to(
        &mut io::stdout().lock(),
        format,
        records,
        summary,
        exit,
    ))
}

fn emit_tests_to(
    writer: &mut impl Write,
    format: OutputFormat,
    records: &[TestRecord],
    summary: &TestSummary,
    exit: ExitCode,
) -> io::Result<ExitCode> {
    match format {
        OutputFormat::Jsonl => {
            for record in records {
                writeln!(writer, "{}", record.to_jsonl())?;
            }
            writeln!(writer, "{}", summary.to_jsonl())?;
        }
        OutputFormat::Text => {
            for record in records {
                writeln!(writer, "{}", record.to_text())?;
            }
            writeln!(writer, "{}", summary.to_text())?;
        }
    }
    writer.flush()?;
    Ok(exit)
}

#[cfg(test)]
mod output_tests {
    use super::*;

    struct FailingWriter {
        remaining: usize,
        bytes: Vec<u8>,
    }

    impl Write for FailingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            let count = self.remaining.min(bytes.len());
            self.bytes.extend_from_slice(&bytes[..count]);
            self.remaining -= count;
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }

    #[test]
    fn failure_records_and_results_stop_at_write_or_flush_failure() {
        let records = [Record::OperationalError {
            code: Code::IoRead,
            detail: None,
        }];
        let tests = [TestRecord {
            name: "passes".into(),
            file: "src/main.mw".into(),
            decl_line: 1,
            decl_column: 1,
            outcome: TestOutcome::Passed,
        }];
        let summary = TestSummary {
            passed: 1,
            failed: 0,
            errored: 0,
            total: 1,
        };
        let palette = Palette::for_test(false);
        for format in [OutputFormat::Text, OutputFormat::Jsonl] {
            for failure_records in [false, true] {
                let mut expected = Vec::new();
                if failure_records {
                    emit_records_to(&mut expected, palette, format, &records, ExitCode::FAILURE)
                } else {
                    emit_tests_to(&mut expected, format, &tests, &summary, ExitCode::SUCCESS)
                }
                .expect("ordinary output succeeds");
                for accepted in [0, 3, usize::MAX] {
                    let mut writer = FailingWriter {
                        remaining: accepted,
                        bytes: Vec::new(),
                    };
                    let error = if failure_records {
                        emit_records_to(&mut writer, palette, format, &records, ExitCode::FAILURE)
                    } else {
                        emit_tests_to(&mut writer, format, &tests, &summary, ExitCode::SUCCESS)
                    }
                    .expect_err("write or final flush fails");
                    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
                    assert_eq!(writer.bytes, expected[..accepted.min(expected.len())]);
                }
            }
        }
    }

    #[test]
    fn run_and_test_have_no_panicking_print_macros() {
        for source in [
            include_str!("cmd_run.rs"),
            include_str!("cmd_test.rs"),
            include_str!("command_output.rs"),
        ] {
            for name in ["print", "println", "eprint", "eprintln"] {
                assert!(!source.contains(&format!("{name}!(")), "{name}");
            }
        }
    }
}
