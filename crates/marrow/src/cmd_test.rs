//! `marrow test [--format text|jsonl] [--filter <substring>]`.
//!
//! Discover `test "name"` declarations from the captured project, compile them
//! into a separately verified image carrying the closed TEST-ENTRY table, and run
//! each through the VM. A storeless test (empty reconstructed demand) runs with no
//! session; a durable test runs against its own fresh ephemeral-memory attachment
//! bounded by the test-image demand union, so tests never observe one another's
//! writes. A passing test reports `passed`; a false `assert` (`run.assert`) reports
//! `failed`; any other runtime fault reports `errored`. Output is a typed
//! `kind: "test"` JSONL stream ending in a summary, or human text. The command
//! exits nonzero when any test fails or errors.

use std::path::PathBuf;
use std::process::ExitCode;

use marrow_codes::Code;
use marrow_compile::{CompileFailure, compile_with_tests};

use crate::outcome::{Record, TestOutcome, TestRecord, TestSummary};
use crate::project::capture_project;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Text,
    Jsonl,
}

struct TestArgs {
    format: Format,
    filter: Option<String>,
}

pub(crate) fn test(rest: &[String]) -> ExitCode {
    let args = match parse_args(rest) {
        Ok(args) => args,
        Err(code) => return code,
    };

    let project = match capture_project(&PathBuf::from(".")) {
        Ok(project) => project,
        Err(failure) => {
            return emit_records(
                args.format,
                &[Record::OperationalError {
                    code: failure.code,
                    detail: Some(failure.message),
                }],
                ExitCode::FAILURE,
            );
        }
    };

    // Family 1: source diagnostics. A malformed test (including an `assert` outside a
    // test) surfaces here, before any image is produced.
    let compiled = match compile_with_tests(&project) {
        Ok(compiled) => compiled,
        Err(CompileFailure::Diagnostics(diagnostics)) => {
            let records: Vec<Record> = diagnostics
                .iter()
                .map(|diagnostic| Record::Diagnostic {
                    code: diagnostic.code,
                    line: diagnostic.line,
                    column: diagnostic.column,
                })
                .collect();
            return emit_records(args.format, &records, ExitCode::FAILURE);
        }
        Err(CompileFailure::ResourceLimit(_)) => {
            return emit_records(
                args.format,
                &[compiler_resource_limit_record()],
                ExitCode::FAILURE,
            );
        }
        Err(CompileFailure::Invariant(_)) => {
            return emit_records(
                args.format,
                &[compiler_invariant_record()],
                ExitCode::FAILURE,
            );
        }
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

    let total = image.test_entries().len();
    let mut records: Vec<TestRecord> = Vec::new();
    let (mut passed, mut failed, mut errored) = (0usize, 0usize, 0usize);

    for entry in image.test_entries() {
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

        // Family 3: a source-mapped runtime fault, or a pass. A storeless test runs
        // with no session. A direct-durable test runs against one harness session over
        // its own fresh attachment. A driver test runs against a fresh persistent
        // attachment, where each export call it makes is its own invocation boundary
        // (a mutating export commits, a later reading export observes it). Every test
        // gets its own attachment, so tests never observe one another's writes. A
        // durable shape the ephemeral kernel does not yet execute is reported as the
        // trough.
        let outcome = match entry.kind() {
            marrow_verify::TestKind::Storeless => {
                classify(marrow_vm::run(&image, entry.func(), Vec::new()))
            }
            marrow_verify::TestKind::DirectDurable => {
                durable_outcome(marrow_vm::run_durable_test(&image, entry), meta)
            }
            marrow_verify::TestKind::Driver => {
                durable_outcome(marrow_vm::run_driver_test(&image, entry), meta)
            }
        };
        match &outcome {
            TestOutcome::Passed => passed += 1,
            TestOutcome::Failed { .. } => failed += 1,
            TestOutcome::Errored { .. } => errored += 1,
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
        return usage("no test matches the filter");
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

fn compiler_invariant_record() -> Record {
    Record::OperationalError {
        code: Code::CliCompilerInvariant.as_str(),
        detail: None,
    }
}

/// The fixed payload-free operational record for a compiler resource-limit outcome:
/// one fixed code, no detail, no source location, no image.
fn compiler_resource_limit_record() -> Record {
    Record::OperationalError {
        code: Code::CliCompilerResourceLimit.as_str(),
        detail: None,
    }
}

/// Map a durable VM run into a test outcome. A run classifies by its result; a
/// durable shape the ephemeral kernel does not yet execute, or an operational mint
/// failure, reports at the test's declaration position.
fn durable_outcome(run: marrow_vm::DurableRun, meta: &marrow_compile::TestEntry) -> TestOutcome {
    match run {
        marrow_vm::DurableRun::Ran(result) => classify(result),
        marrow_vm::DurableRun::Parked => TestOutcome::Errored {
            code: Code::CliDurableUnsupported.as_str(),
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
/// false `assert` (`run.assert`) fails, and any other source-mapped runtime fault
/// errors.
fn classify(result: Result<Option<marrow_vm::Value>, marrow_vm::RuntimeFault>) -> TestOutcome {
    match result {
        Ok(_) => TestOutcome::Passed,
        Err(fault) if fault.code() == Code::RunAssert.as_str() => TestOutcome::Failed {
            code: fault.code(),
            line: fault.line(),
            column: fault.column(),
        },
        Err(fault) => TestOutcome::Errored {
            code: fault.code(),
            line: fault.line(),
            column: fault.column(),
        },
    }
}

fn parse_args(rest: &[String]) -> Result<TestArgs, ExitCode> {
    let mut format = Format::Text;
    let mut filter: Option<String> = None;
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--format" => match iter.next().map(String::as_str) {
                Some("jsonl") => format = Format::Jsonl,
                Some("text") => format = Format::Text,
                _ => return Err(usage("`--format` must be `text` or `jsonl`")),
            },
            "--filter" => match iter.next() {
                Some(value) => filter = Some(value.clone()),
                None => return Err(usage("`--filter` needs a substring")),
            },
            other => return Err(usage(&format!("unknown test option: {other}"))),
        }
    }
    Ok(TestArgs { format, filter })
}

fn usage(message: &str) -> ExitCode {
    eprintln!("{message}; run marrow --help for usage");
    ExitCode::from(2)
}

/// Emit typed failure records (capture/compile/verify) and return `exit`.
fn emit_records(format: Format, records: &[Record], exit: ExitCode) -> ExitCode {
    // The test command's typed failure records are never a value, so they carry no
    // record types to render.
    for record in records {
        match format {
            Format::Jsonl => println!("{}", record.to_jsonl(&[], &[])),
            Format::Text => {
                let text = record.to_text(&[], &[]);
                if !text.is_empty() {
                    println!("{text}");
                }
            }
        }
    }
    exit
}

/// Emit each test record then the summary in the selected format, returning `exit`.
fn emit_tests(
    format: Format,
    records: &[TestRecord],
    summary: &TestSummary,
    exit: ExitCode,
) -> ExitCode {
    match format {
        Format::Jsonl => {
            for record in records {
                println!("{}", record.to_jsonl());
            }
            println!("{}", summary.to_jsonl());
        }
        Format::Text => {
            for record in records {
                println!("{}", record.to_text());
            }
            println!("{}", summary.to_text());
        }
    }
    exit
}

#[cfg(test)]
mod compiler_invariant_tests {
    #[test]
    fn invariant_mapper_is_one_payload_free_operational_record() {
        assert_eq!(
            super::compiler_invariant_record(),
            super::Record::OperationalError {
                code: marrow_codes::Code::CliCompilerInvariant.as_str(),
                detail: None,
            }
        );
    }
}
