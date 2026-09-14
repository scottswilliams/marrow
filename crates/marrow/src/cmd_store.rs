//! Terminal store operations delegate once to the release-verified companion.
//! Image-based operations compile without opening the store or minting identities.
//! Restore uses its backup's image and needs no project capture or compilation.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use marrow_compile::{CompileFailure, compile};

use crate::project::capture_project;

#[derive(Clone, Copy)]
pub(crate) enum Operation {
    Doctor,
    Recover,
    Backup,
    Restore,
    Apply,
}

impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Doctor => "doctor",
            Self::Recover => "recover",
            Self::Backup => "backup",
            Self::Restore => "restore",
            Self::Apply => "apply",
        }
    }
    fn runner_command(self) -> &'static str {
        match self {
            Self::Doctor => "audit",
            Self::Recover => "recover",
            Self::Backup => "backup",
            Self::Restore => "restore",
            Self::Apply => "apply",
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
    action: Action,
}

enum Action {
    Inspect,
    Backup(PathBuf),
    Restore(PathBuf),
    Apply {
        old: PathBuf,
        new: PathBuf,
        ceiling: Option<String>,
    },
    RecoverImage(PathBuf),
}

pub(crate) fn run(operation: Operation, rest: &[String]) -> ExitCode {
    let args = match parse_args(operation, rest) {
        Ok(args) => args,
        Err(code) => return code,
    };

    if matches!(
        &args.action,
        Action::Restore(_) | Action::Apply { .. } | Action::RecoverImage(_)
    ) {
        let mut command = match companion_command(operation, &args) {
            Ok(command) => command,
            Err(code) => return code,
        };
        match &args.action {
            Action::Restore(input) => {
                command.arg("--from").arg(input);
            }
            Action::RecoverImage(image) => {
                command.arg("--image").arg(image);
            }
            Action::Apply { old, new, ceiling } => {
                command
                    .arg("--old-image")
                    .arg(old)
                    .arg("--new-image")
                    .arg(new);
                if let Some(id) = ceiling {
                    command.arg("--accept-ceiling").arg(id);
                }
            }
            _ => unreachable!("explicit-artifact action checked above"),
        }
        return run_companion(command);
    }

    let project = match capture_project(&PathBuf::from(".")) {
        Ok(project) => project,
        Err(failure) => {
            crate::report_simple_error(failure.code, &failure.message);
            return ExitCode::FAILURE;
        }
    };

    // Image-based operations require the stored program's committed identities.
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

    let image = match crate::companion::stage_image(operation.name(), &compiled.image.bytes) {
        Ok(image) => image,
        Err(err) => {
            crate::report_simple_error(marrow_codes::Code::IoWrite.as_str(), &err.to_string());
            return ExitCode::FAILURE;
        }
    };

    let mut command = match companion_command(operation, &args) {
        Ok(command) => command,
        Err(code) => return code,
    };
    command.arg("--image").arg(image.path());
    if let Action::Backup(output) = &args.action {
        command.arg("--out").arg(output);
    }
    run_companion(command)
}

fn companion_command(operation: Operation, args: &Args) -> Result<Command, ExitCode> {
    let runner = crate::companion::discover_companion().map_err(|damage| {
        crate::report_simple_error(
            marrow_codes::Code::CliInstallationDamaged.as_str(),
            damage.message(),
        );
        ExitCode::FAILURE
    })?;
    let mut command = Command::new(runner);
    command
        .arg(operation.runner_command())
        .arg("--store")
        .arg(&args.store)
        .arg("--format")
        .arg(match args.format {
            Format::Text => "text",
            Format::Jsonl => "jsonl",
        });
    Ok(command)
}

fn run_companion(mut command: Command) -> ExitCode {
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
    let mut transfer = None;
    let mut seen_format = false;
    let mut old = None;
    let mut new = None;
    let mut ceiling = None;
    let mut selected_image = None;
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--old-image" if matches!(operation, Operation::Apply) && old.is_none() => {
                old = Some(PathBuf::from(next_value(
                    operation,
                    &mut iter,
                    "--old-image",
                )?));
            }
            "--new-image" if matches!(operation, Operation::Apply) && new.is_none() => {
                new = Some(PathBuf::from(next_value(
                    operation,
                    &mut iter,
                    "--new-image",
                )?));
            }
            "--accept-ceiling" if matches!(operation, Operation::Apply) && ceiling.is_none() => {
                ceiling = Some(next_value(operation, &mut iter, "--accept-ceiling")?);
            }
            "--image" if matches!(operation, Operation::Recover) && selected_image.is_none() => {
                selected_image = Some(PathBuf::from(next_value(operation, &mut iter, "--image")?));
            }
            "--store" if store.is_none() => {
                store = Some(PathBuf::from(next_value(operation, &mut iter, "--store")?))
            }
            "--out" if matches!(operation, Operation::Backup) && transfer.is_none() => {
                transfer = Some(PathBuf::from(next_value(operation, &mut iter, "--out")?))
            }
            "--from" if matches!(operation, Operation::Restore) && transfer.is_none() => {
                transfer = Some(PathBuf::from(next_value(operation, &mut iter, "--from")?))
            }
            "--format" if !seen_format => {
                seen_format = true;
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
    let action = match operation {
        Operation::Doctor => Action::Inspect,
        Operation::Recover => selected_image.map_or(Action::Inspect, Action::RecoverImage),
        Operation::Apply => Action::Apply {
            old: old
                .ok_or_else(|| usage(operation, "`--old-image` must name the active artifact"))?,
            new: new
                .ok_or_else(|| usage(operation, "`--new-image` must name the selected artifact"))?,
            ceiling,
        },
        Operation::Backup => Action::Backup(
            transfer.ok_or_else(|| usage(operation, "`--out` must name the backup file"))?,
        ),
        Operation::Restore => Action::Restore(
            transfer.ok_or_else(|| usage(operation, "`--from` must name the backup file"))?,
        ),
    };
    Ok(Args {
        store,
        format,
        action,
    })
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
    let transfer = match operation {
        Operation::Backup => " --out <backup>",
        Operation::Restore => " --from <backup>",
        Operation::Apply => " --old-image <old.mwi> --new-image <new.mwi> [--accept-ceiling <id>]",
        Operation::Recover => " [--image <image.mwi>]",
        _ => "",
    };
    eprintln!(
        "{message}\nusage: marrow {} --store <dir>{transfer} [--format text|jsonl]",
        operation.name()
    );
    ExitCode::from(2)
}
