//! `marrow doctor --store <dir> [--format text|jsonl]`: audit a store read-only against the
//! program it is bound to.
//!
//! The terminal compiles the project at the working directory, exactly like `marrow run
//! --store`, and never opens the store itself: it writes the compiled image to a private
//! temporary file and hands the audit to the release-verified companion runner
//! (`marrow-runner audit`), the sole opener of the store, which prints the report in the
//! requested format. The runner takes the store's owner lock, admits the image as the
//! store's exact active binding, runs the engine's integrity audit and the kernel's bounded
//! logical walk, and releases the lock; a code-only edit the store has not been rebound to
//! is `store.image_not_active`. The exit code is the runner's: `0` for a clean store, `1`
//! for findings, a corrupt engine, or a refusal.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use marrow_compile::{CompileFailure, compile};

use crate::project::capture_project;

/// The output format for `marrow doctor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Text,
    Jsonl,
}

struct Args {
    store: PathBuf,
    format: Format,
}

pub(crate) fn doctor(rest: &[String]) -> ExitCode {
    let args = match parse_args(rest) {
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

    // Compile without opening a store and without minting: the audit compares the store with
    // the committed program, so a missing identity points at `marrow check`.
    let compiled = match compile(&project) {
        Ok(compiled) => compiled,
        Err(CompileFailure::Diagnostics(diagnostics)) => {
            for diagnostic in diagnostics.iter() {
                eprintln!("{}: {}", diagnostic.code(), diagnostic.message());
            }
            eprintln!("the project does not compile; run `marrow check` before auditing a store");
            return ExitCode::FAILURE;
        }
        Err(_) => {
            crate::report_simple_error(
                marrow_codes::Code::ConfigInvalid.as_str(),
                "the project could not be compiled; run `marrow check` before auditing a store",
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

    let image = match crate::companion::stage_image("doctor", &compiled.image.bytes) {
        Ok(image) => image,
        Err(err) => {
            crate::report_simple_error(marrow_codes::Code::IoWrite.as_str(), &err.to_string());
            return ExitCode::FAILURE;
        }
    };

    let mut command = Command::new(&runner);
    command
        .arg("audit")
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

fn parse_args(rest: &[String]) -> Result<Args, ExitCode> {
    let mut store: Option<PathBuf> = None;
    let mut format = Format::Text;
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--store" => store = Some(PathBuf::from(next_value(&mut iter, "--store")?)),
            "--format" => {
                format = match next_value(&mut iter, "--format")?.as_str() {
                    "text" => Format::Text,
                    "jsonl" => Format::Jsonl,
                    other => return Err(usage(&format!("unknown format `{other}`"))),
                }
            }
            other => return Err(crate::unknown_option("doctor", other)),
        }
    }
    let Some(store) = store else {
        return Err(usage("`--store` names the store directory to audit"));
    };
    Ok(Args { store, format })
}

fn next_value(iter: &mut std::slice::Iter<'_, String>, flag: &str) -> Result<String, ExitCode> {
    match iter.next() {
        Some(value) => Ok(value.clone()),
        None => Err(usage(&format!("`{flag}` needs a value"))),
    }
}

fn usage(message: &str) -> ExitCode {
    eprintln!("{message}\nusage: marrow doctor --store <dir> [--format text|jsonl]");
    ExitCode::from(2)
}
