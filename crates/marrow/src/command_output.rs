//! The command-line surface every command shares: the `--format` value, the one
//! usage-refusal form, the flag-value cursor, and delivery-failure reporting.

use std::io::{self, Write};
use std::process::ExitCode;

use crate::Command;

/// The output format a command renders: human-shaped text, or one JSON object per line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    Text,
    Jsonl,
}

impl OutputFormat {
    /// The `--format` spelling, also what the companion runner takes.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Jsonl => "jsonl",
        }
    }
}

/// A usage refusal: the specific problem, then the command's own usage. Exit 2 keeps a
/// mistyped invocation from passing a CI gate green. Written without a panicking print
/// macro, so a closed diagnostic channel refuses rather than aborts.
pub(crate) fn usage(command: Command, message: &str) -> ExitCode {
    refusal(&format!("marrow {}", command.name()), message)
}

/// A refusal of the command line before any command was selected.
pub(crate) fn top_level_usage(message: &str) -> ExitCode {
    refusal("marrow", message)
}

fn refusal(spelling: &str, message: &str) -> ExitCode {
    let _ = writeln!(
        io::stderr().lock(),
        "{message}; run {spelling} --help for usage"
    );
    ExitCode::from(2)
}

pub(crate) fn unknown_option(command: Command, option: &str) -> ExitCode {
    usage(command, &format!("unknown option `{option}`"))
}

/// The value a flag takes as its next argument, as in `--store ./store`.
pub(crate) fn flag_value<'a>(
    args: &mut impl Iterator<Item = &'a String>,
    command: Command,
    flag: &str,
) -> Result<&'a str, ExitCode> {
    args.next()
        .map(String::as_str)
        .ok_or_else(|| usage(command, &format!("`{flag}` needs a value")))
}

/// The `--format` flag's value.
pub(crate) fn format_flag<'a>(
    args: &mut impl Iterator<Item = &'a String>,
    command: Command,
) -> Result<OutputFormat, ExitCode> {
    match flag_value(args, command, "--format")? {
        "text" => Ok(OutputFormat::Text),
        "jsonl" => Ok(OutputFormat::Jsonl),
        _ => Err(usage(command, "`--format` must be `text` or `jsonl`")),
    }
}

/// Record `value` into `slot`, refusing a second one. `what` names what the command
/// takes one of, so the refusal reads `marrow check takes one project directory`.
pub(crate) fn once<T>(
    slot: &mut Option<T>,
    value: T,
    command: Command,
    what: &str,
) -> Result<(), ExitCode> {
    if slot.replace(value).is_some() {
        return Err(usage(
            command,
            &format!("marrow {} takes one {what}", command.name()),
        ));
    }
    Ok(())
}

/// Delivery can fail after execution completed. Report it without replaying the
/// command, even when its diagnostic channel is also unavailable.
pub(crate) fn finish(result: io::Result<ExitCode>) -> ExitCode {
    match result {
        Ok(exit) => exit,
        Err(error) => {
            let _ = writeln!(
                io::stderr().lock(),
                "{}: {error}",
                marrow_codes::Code::IoWrite.as_str()
            );
            ExitCode::FAILURE
        }
    }
}
