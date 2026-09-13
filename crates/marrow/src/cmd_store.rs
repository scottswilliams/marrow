//! Terminal store inspection and recovery. Compile without opening the store or
//! minting identities, stage the image, and delegate once to the release-verified
//! companion. Doctor remains a read-only logical inspection; explicit recovery
//! validates physical and logical integrity and establishes fresh activation.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use marrow_compile::{CompileFailure, compile};

use crate::project::capture_project;

#[derive(Clone, Copy)]
pub(crate) enum Operation {
    Doctor,
    Recover,
}

impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Doctor => "doctor",
            Self::Recover => "recover",
        }
    }
    fn runner_command(self) -> &'static str {
        match self {
            Self::Doctor => "audit",
            Self::Recover => "recover",
        }
    }
}

/// The output format requested from the companion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Text,
    Jsonl,
}

struct Args {
    store: PathBuf,
    format: Format,
}

pub(crate) fn run(operation: Operation, rest: &[String]) -> ExitCode {
    let args = match parse_args(operation, rest) {
        Ok(args) => args,
        Err(code) => return code,
    };

    let project = match capture_project(&PathBuf::from(".")) {
        Ok(project) => project,
        Err(failure) => {
            crate::report_simple_error(failure.code, &failure.message);
            return ExitCode::FAILURE;
        }
    };

    // Both operations require the committed identities of the stored program.
    let compiled = match compile(&project) {
        Ok(compiled) => compiled,
        Err(CompileFailure::Diagnostics(diagnostics)) => {
            for diagnostic in diagnostics.iter() {
                eprintln!("{}: {}", diagnostic.code(), diagnostic.message());
            }
            eprintln!("the project does not compile; run `marrow check` before accessing a store");
            return ExitCode::FAILURE;
        }
        Err(CompileFailure::ResourceLimit(limit)) => {
            crate::report_simple_error(
                marrow_codes::Code::CliCompilerResourceLimit.as_str(),
                &crate::resource_limit_message(limit.kind().description()),
            );
            return ExitCode::FAILURE;
        }
        Err(CompileFailure::Invariant(_)) => {
            crate::report_simple_error(
                marrow_codes::Code::CliCompilerInvariant.as_str(),
                "the compiler failed an internal consistency check",
            );
            return ExitCode::FAILURE;
        }
    };

    let runner = match crate::companion::discover_companion() {
        Ok(runner) => runner,
        Err(damage) => {
            crate::report_simple_error(
                marrow_codes::Code::CliInstallationDamaged.as_str(),
                damage.message(),
            );
            return ExitCode::FAILURE;
        }
    };

    let image = match crate::companion::stage_image(operation.name(), &compiled.image.bytes) {
        Ok(image) => image,
        Err(err) => {
            crate::report_simple_error(marrow_codes::Code::IoWrite.as_str(), &err.to_string());
            return ExitCode::FAILURE;
        }
    };

    let mut command = Command::new(&runner);
    command
        .arg(operation.runner_command())
        .arg("--image")
        .arg(image.path())
        .arg("--store")
        .arg(&args.store)
        .arg("--format")
        .arg(match args.format {
            Format::Text => "text",
            Format::Jsonl => "jsonl",
        });

    match command.status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(err) => {
            crate::report_simple_error(marrow_codes::Code::RunnerSpawn.as_str(), &err.to_string());
            ExitCode::FAILURE
        }
    }
}

fn parse_args(operation: Operation, rest: &[String]) -> Result<Args, ExitCode> {
    let mut store: Option<PathBuf> = None;
    let mut format = Format::Text;
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--store" => store = Some(PathBuf::from(next_value(operation, &mut iter, "--store")?)),
            "--format" => {
                format = match next_value(operation, &mut iter, "--format")?.as_str() {
                    "text" => Format::Text,
                    "jsonl" => Format::Jsonl,
                    other => return Err(usage(operation, &format!("unknown format `{other}`"))),
                }
            }
            other => return Err(crate::unknown_option(operation.name(), other)),
        }
    }
    let Some(store) = store else {
        return Err(usage(operation, "`--store` must name the store directory"));
    };
    Ok(Args { store, format })
}

fn next_value(
    operation: Operation,
    iter: &mut std::slice::Iter<'_, String>,
    flag: &str,
) -> Result<String, ExitCode> {
    match iter.next() {
        Some(value) => Ok(value.clone()),
        None => Err(usage(operation, &format!("`{flag}` needs a value"))),
    }
}

fn usage(operation: Operation, message: &str) -> ExitCode {
    eprintln!(
        "{message}\nusage: marrow {} --store <dir> [--format text|jsonl]",
        operation.name()
    );
    ExitCode::from(2)
}
