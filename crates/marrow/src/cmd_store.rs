//! Terminal store operations delegate once to the release-verified companion.
//! Image-based operations compile without opening the store or minting identities.
//! Restore uses its backup's image and needs no project capture or compilation.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use marrow_compile::compile;

use crate::companion::{companion_command, run_companion, stage_image};
use crate::project::compile_project;

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

impl Action {
    /// The companion flags naming artifacts the command line supplied, when it supplied any.
    /// `Some` means the operation runs entirely from those artifacts: the terminal captures
    /// and compiles no project and mints no identities.
    fn explicit_artifacts(&self) -> Option<Vec<&OsStr>> {
        match self {
            Self::Inspect | Self::Backup(_) => None,
            Self::Restore(input) => Some(vec![OsStr::new("--from"), input.as_os_str()]),
            Self::RecoverImage(image) => Some(vec![OsStr::new("--image"), image.as_os_str()]),
            Self::Apply { old, new, ceiling } => {
                let mut flags = vec![
                    OsStr::new("--old-image"),
                    old.as_os_str(),
                    OsStr::new("--new-image"),
                    new.as_os_str(),
                ];
                if let Some(id) = ceiling {
                    flags.extend([OsStr::new("--accept-ceiling"), OsStr::new(id)]);
                }
                Some(flags)
            }
        }
    }
}

pub(crate) fn run(operation: Operation, rest: &[String]) -> ExitCode {
    let args = match parse_args(operation, rest) {
        Ok(args) => args,
        Err(code) => return code,
    };

    if let Some(artifacts) = args.action.explicit_artifacts() {
        let mut command = match store_command(operation, &args) {
            Ok(command) => command,
            Err(code) => return code,
        };
        command.args(artifacts);
        return run_companion(command);
    }

    // Image-based operations require the stored program's committed identities.
    let compiled = match compile_project(
        Path::new("."),
        compile,
        Some("the project does not compile; run `marrow check` before accessing a store"),
    ) {
        Ok((compiled, _)) => compiled,
        Err(code) => return code,
    };

    let image = match stage_image(&compiled.image.bytes) {
        Ok(image) => image,
        Err(code) => return code,
    };

    let mut command = match store_command(operation, &args) {
        Ok(command) => command,
        Err(code) => return code,
    };
    command.arg("--image").arg(image.path());
    if let Action::Backup(output) = &args.action {
        command.arg("--out").arg(output);
    }
    run_companion(command)
}

fn store_command(operation: Operation, args: &Args) -> Result<Command, ExitCode> {
    let mut command = companion_command(operation.runner_command())?;
    command
        .arg("--store")
        .arg(&args.store)
        .arg("--format")
        .arg(match args.format {
            Format::Text => "text",
            Format::Jsonl => "jsonl",
        });
    Ok(command)
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
