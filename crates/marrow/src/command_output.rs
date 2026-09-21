//! The command-line surface every command shares: the `--format` value, the one
//! usage-refusal form, the argument cursor, and delivery-failure reporting.

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

/// The cursor a command reads its arguments through: the remaining arguments and the
/// command whose usage a refusal names.
pub(crate) struct Flags<'a> {
    args: std::slice::Iter<'a, String>,
    command: Command,
}

impl<'a> Iterator for Flags<'a> {
    type Item = &'a String;

    fn next(&mut self) -> Option<&'a String> {
        self.args.next()
    }
}

impl<'a> Flags<'a> {
    pub(crate) fn new(rest: &'a [String], command: Command) -> Self {
        Self {
            args: rest.iter(),
            command,
        }
    }

    /// The value `flag` takes as its next argument, as in `--store ./store`.
    fn value(&mut self, flag: &str) -> Result<&'a str, ExitCode> {
        self.args
            .next()
            .map(String::as_str)
            .ok_or_else(|| usage(self.command, &format!("`{flag}` needs a value")))
    }

    /// Read `flag`'s value through `parse` into `slot`, refusing a missing value and a
    /// repeated flag.
    pub(crate) fn read<T>(
        &mut self,
        slot: &mut Option<T>,
        flag: &str,
        parse: impl FnOnce(&'a str) -> T,
    ) -> Result<(), ExitCode> {
        let value = parse(self.value(flag)?);
        once(slot, value, self.command, &format!("`{flag}`"))
    }

    /// Read the `--format` value into `slot`.
    pub(crate) fn read_format(&mut self, slot: &mut Option<OutputFormat>) -> Result<(), ExitCode> {
        let format = match self.value("--format")? {
            "text" => OutputFormat::Text,
            "jsonl" => OutputFormat::Jsonl,
            _ => return Err(usage(self.command, "`--format` must be `text` or `jsonl`")),
        };
        once(slot, format, self.command, "`--format`")
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
