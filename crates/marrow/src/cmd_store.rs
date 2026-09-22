//! Terminal store operations delegate once to the release-verified companion.
//! Operations without an explicit image compile without opening the store or minting identities.
//! Restore uses its backup's image and needs no project capture or compilation.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, ExitCode};

use marrow_compile::compile;

use crate::Command;
use crate::command_output::{Flags, OutputFormat, unknown_option, usage};
use crate::companion::{companion_command, run_companion, stage_image};
use crate::project::compile_project;

pub(crate) const DOCTOR_HELP: &str = "\
Usage:
  marrow doctor --store <dir> [--format text|jsonl]

Compile and verify the project at the working directory, which must be the store's
active program, then audit the store read-only through the companion runner: count
logical findings, list at most 256 of them, and report an entry-content digest.
Physical integrity is not checked. Exit 0 means the walk found no logical
inconsistency; findings, engine errors, and refusals exit 1.
";

pub(crate) const APPLY_HELP: &str = "\
Usage:
  marrow apply --store <dir> --old-image <image> --new-image <image> [--accept-ceiling <id>] [--format text|jsonl]

Verify the explicit old and new image artifacts without capturing a project, then
move the store from the old image, which must be its exact active binding, to the
new one. Every old durable representation is preserved; new sparse scalar fields
start absent. An authority expansion requires --accept-ceiling to name the exact
proposed ceiling, which the refusal prints.
";

pub(crate) const RECOVER_HELP: &str = "\
Usage:
  marrow recover --store <dir> [--image <path>] [--format text|jsonl]

Validate and activate the store's actual stored head through the companion runner,
using the selected image artifact, or the compiled project at the working directory
when none is given. Recovery validates physical and logical integrity and runs no
export.
";

pub(crate) const BACKUP_HELP: &str = "\
Usage:
  marrow backup --store <dir> --out <backup> [--image <path>] [--format text|jsonl]

Export the store's exact contents with its active image into <backup> through the
companion runner. Use the selected image artifact, or compile the project at the
working directory when none is given. The image must match the store's active
program. The destination must not exist; its parent directory must already exist.
";

pub(crate) const RESTORE_HELP: &str = "\
Usage:
  marrow restore --from <backup> --store <dir> [--format text|jsonl]

Construct a fresh store at <dir> from the backup's embedded verified image through
the companion runner. No project is compiled. The destination must not exist;
its parent directory must already exist.
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operation {
    Doctor,
    Recover,
    Backup,
    Restore,
    Apply,
}

impl Operation {
    fn command(self) -> Command {
        match self {
            Self::Doctor => Command::Doctor,
            Self::Recover => Command::Recover,
            Self::Backup => Command::Backup,
            Self::Restore => Command::Restore,
            Self::Apply => Command::Apply,
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

struct Args {
    store: PathBuf,
    format: OutputFormat,
    action: Action,
}

enum Action {
    Inspect,
    Backup(PathBuf),
    BackupImage {
        output: PathBuf,
        image: PathBuf,
    },
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
            Self::BackupImage { output, image } => Some(vec![
                OsStr::new("--image"),
                image.as_os_str(),
                OsStr::new("--out"),
                output.as_os_str(),
            ]),
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
        Ok(compiled) => compiled,
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

fn store_command(operation: Operation, args: &Args) -> Result<Process, ExitCode> {
    let mut command = companion_command(operation.runner_command())?;
    command
        .arg("--store")
        .arg(&args.store)
        .arg("--format")
        .arg(args.format.as_str());
    Ok(command)
}

fn parse_args(operation: Operation, rest: &[String]) -> Result<Args, ExitCode> {
    let command = operation.command();
    let mut store: Option<PathBuf> = None;
    let mut format: Option<OutputFormat> = None;
    let mut transfer: Option<PathBuf> = None;
    let mut old: Option<PathBuf> = None;
    let mut new: Option<PathBuf> = None;
    let mut ceiling: Option<String> = None;
    let mut selected_image: Option<PathBuf> = None;
    let mut flags = Flags::new(rest, command);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--old-image" if operation == Operation::Apply => {
                flags.read(&mut old, "--old-image", PathBuf::from)?
            }
            "--new-image" if operation == Operation::Apply => {
                flags.read(&mut new, "--new-image", PathBuf::from)?
            }
            "--accept-ceiling" if operation == Operation::Apply => {
                flags.read(&mut ceiling, "--accept-ceiling", str::to_string)?
            }
            "--image" if matches!(operation, Operation::Recover | Operation::Backup) => {
                flags.read(&mut selected_image, "--image", PathBuf::from)?
            }
            "--store" => flags.read(&mut store, "--store", PathBuf::from)?,
            "--out" if operation == Operation::Backup => {
                flags.read(&mut transfer, "--out", PathBuf::from)?
            }
            "--from" if operation == Operation::Restore => {
                flags.read(&mut transfer, "--from", PathBuf::from)?
            }
            "--format" => flags.read_format(&mut format)?,
            other => return Err(unknown_option(command, other)),
        }
    }
    let store = store.ok_or_else(|| usage(command, "`--store` must name the store directory"))?;
    let action = match operation {
        Operation::Doctor => Action::Inspect,
        Operation::Recover => selected_image.map_or(Action::Inspect, Action::RecoverImage),
        Operation::Apply => Action::Apply {
            old: old
                .ok_or_else(|| usage(command, "`--old-image` must name the active artifact"))?,
            new: new
                .ok_or_else(|| usage(command, "`--new-image` must name the selected artifact"))?,
            ceiling,
        },
        Operation::Backup => {
            let output =
                transfer.ok_or_else(|| usage(command, "`--out` must name the backup file"))?;
            match selected_image {
                Some(image) => Action::BackupImage { output, image },
                None => Action::Backup(output),
            }
        }
        Operation::Restore => Action::Restore(
            transfer.ok_or_else(|| usage(command, "`--from` must name the backup file"))?,
        ),
    };
    Ok(Args {
        store,
        format: format.unwrap_or(OutputFormat::Text),
        action,
    })
}
