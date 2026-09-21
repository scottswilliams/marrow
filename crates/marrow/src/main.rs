use marrow_codes::Code;
use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

use crate::term_style::{Palette, Stream, Style};

mod cmd_check;
mod cmd_client;
mod cmd_fmt;
mod cmd_image;
mod cmd_import;
mod cmd_init;
mod cmd_run;
mod cmd_store;
mod cmd_test;
mod command_output;
mod companion;
mod demand;
mod outcome;
mod project;
mod term_style;
mod tsgen;

const HELP: &str = "\
Marrow

Usage:
  marrow init <projectdir>
  marrow fmt [--check | --write] <file.mw | projectdir>
  marrow check [projectdir]
  marrow run <export> [--stdin] [--store <dir>] [--format text|jsonl] [-- <args>...]
  marrow import --store <dir> --jsonl <path> --root <name> [--keys <col,...>]
  marrow doctor --store <dir> [--format text|jsonl]
  marrow apply --store <dir> --old-image <image> --new-image <image> [--accept-ceiling <id>] [--format text|jsonl]
  marrow recover --store <dir> [--image <path>] [--format text|jsonl]
  marrow backup --store <dir> --out <backup> [--format text|jsonl]
  marrow restore --from <backup> --store <dir> [--format text|jsonl]
  marrow test [--format text|jsonl] [--filter <substring>]
  marrow client typescript [--out <dir>]
  marrow image --out <dir> --accept-ceiling <id>
  marrow --version
  marrow --help
  marrow <command> --help

Run `marrow <command> --help` for what a command does and how it exits.
";

fn main() -> ExitCode {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let Some((command, rest)) = args.split_first() else {
        // A bare `marrow` is a usage error, not success: it ran no command. Printing usage to
        // stderr and exiting 2 keeps a forgotten subcommand from passing a CI gate green.
        eprint!("{}", term_style::render_help(Stream::Stderr, HELP));
        return ExitCode::from(2);
    };
    // Parsing recurses over the source on the call stack, so dispatch on a worker
    // thread with a generous stack. The parser's recursion guard trips far inside
    // this stack, so deeply nested source surfaces a typed `check.nesting_limit`
    // diagnostic instead of aborting the process with a native stack overflow.
    let command = command.clone();
    let rest = rest.to_vec();
    run_on_worker_stack(move || dispatch_os(&command, &rest))
}

fn dispatch_os(command: &OsStr, rest: &[OsString]) -> ExitCode {
    let Some(name) = command.to_str() else {
        return command_output::top_level_usage(&format!(
            "unknown command `{}`",
            command.to_string_lossy()
        ));
    };
    match name {
        "--help" | "-h" | "help" => {
            print!("{}", term_style::render_help(Stream::Stdout, HELP));
            ExitCode::SUCCESS
        }
        "--version" | "-V" | "version" => {
            println!(
                "{} {}",
                term_style::paint(Stream::Stdout, Style::Code, "marrow"),
                env!("CARGO_PKG_VERSION"),
            );
            ExitCode::SUCCESS
        }
        name => match Command::parse(name) {
            // Usage is answered before the arguments are read as text, so an
            // undecodable argument beside `--help` still gets the usage.
            Some(command) if asks_for_help(rest) => {
                print!("{}", command.help());
                ExitCode::SUCCESS
            }
            Some(command) => match utf8_args(rest) {
                Some(rest) => command.run(&rest),
                None => {
                    report_simple_error(
                        Code::ConfigInvalid,
                        "command arguments must be valid UTF-8",
                    );
                    ExitCode::FAILURE
                }
            },
            None => command_output::top_level_usage(&format!("unknown command `{name}`")),
        },
    }
}

/// Whether the arguments ask for the command's usage: `--help` or `-h` anywhere before
/// a `--` separator. What follows the separator belongs to the export being run.
fn asks_for_help(rest: &[OsString]) -> bool {
    rest.iter()
        .take_while(|arg| arg.as_os_str() != "--")
        .any(|arg| arg == "--help" || arg == "-h")
}

fn utf8_args(args: &[OsString]) -> Option<Vec<String>> {
    args.iter()
        .map(|arg| arg.to_str().map(str::to_string))
        .collect()
}

/// The commands `marrow` dispatches, one per subcommand name. `--help`/`-h` is
/// answered here for every command before its own options are read, so each command
/// accepts it by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    Init,
    Fmt,
    Check,
    Run,
    Import,
    Doctor,
    Apply,
    Recover,
    Backup,
    Restore,
    Test,
    Client,
    Image,
}

impl Command {
    /// Every command, in usage order. Membership here is what makes a command
    /// reachable: `parse` finds names only in this table.
    const ALL: [Command; 13] = [
        Command::Init,
        Command::Fmt,
        Command::Check,
        Command::Run,
        Command::Import,
        Command::Doctor,
        Command::Apply,
        Command::Recover,
        Command::Backup,
        Command::Restore,
        Command::Test,
        Command::Client,
        Command::Image,
    ];

    fn parse(name: &str) -> Option<Command> {
        Command::ALL
            .into_iter()
            .find(|command| command.name() == name)
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Command::Init => "init",
            Command::Fmt => "fmt",
            Command::Check => "check",
            Command::Run => "run",
            Command::Import => "import",
            Command::Doctor => "doctor",
            Command::Apply => "apply",
            Command::Recover => "recover",
            Command::Backup => "backup",
            Command::Restore => "restore",
            Command::Test => "test",
            Command::Client => "client",
            Command::Image => "image",
        }
    }

    fn help(self) -> &'static str {
        match self {
            Command::Init => cmd_init::HELP,
            Command::Fmt => cmd_fmt::HELP,
            Command::Check => cmd_check::HELP,
            Command::Run => cmd_run::HELP,
            Command::Import => cmd_import::HELP,
            Command::Doctor => cmd_store::DOCTOR_HELP,
            Command::Apply => cmd_store::APPLY_HELP,
            Command::Recover => cmd_store::RECOVER_HELP,
            Command::Backup => cmd_store::BACKUP_HELP,
            Command::Restore => cmd_store::RESTORE_HELP,
            Command::Test => cmd_test::HELP,
            Command::Client => cmd_client::HELP,
            Command::Image => cmd_image::HELP,
        }
    }

    fn run(self, rest: &[String]) -> ExitCode {
        match self {
            Command::Init => cmd_init::init(rest),
            Command::Fmt => cmd_fmt::fmt(rest),
            Command::Check => cmd_check::check(rest),
            Command::Run => cmd_run::run(rest),
            Command::Import => cmd_import::import(rest),
            Command::Doctor => cmd_store::run(cmd_store::Operation::Doctor, rest),
            Command::Apply => cmd_store::run(cmd_store::Operation::Apply, rest),
            Command::Recover => cmd_store::run(cmd_store::Operation::Recover, rest),
            Command::Backup => cmd_store::run(cmd_store::Operation::Backup, rest),
            Command::Restore => cmd_store::run(cmd_store::Operation::Restore, rest),
            Command::Test => cmd_test::test(rest),
            Command::Client => cmd_client::client(rest),
            Command::Image => cmd_image::image(rest),
        }
    }
}

/// The stack the parse/format pipeline runs on. 256 MiB comfortably holds the
/// recursion the typed parser limit permits — 256 nested parser frames on every
/// path that recurses, whether or not it opens a brace — with wide margin, so the
/// limit always trips before the stack does, at any admitted file length.
const WORKER_STACK_BYTES: usize = 256 * 1024 * 1024;

/// Run `command` on a worker thread with [`WORKER_STACK_BYTES`] of stack and
/// return its exit code. The main thread only waits, so the deep recursion the
/// parser performs over untrusted source has room to reach a typed depth-limit
/// diagnostic rather than overflowing the default main-thread stack.
fn run_on_worker_stack(command: impl FnOnce() -> ExitCode + Send + 'static) -> ExitCode {
    let worker = std::thread::Builder::new()
        .stack_size(WORKER_STACK_BYTES)
        .spawn(command);
    run_worker_thread(worker)
}

fn run_worker_thread(worker: std::io::Result<std::thread::JoinHandle<ExitCode>>) -> ExitCode {
    match worker {
        Ok(worker) => worker
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
        Err(error) => {
            report_simple_error(
                Code::IoThread,
                &format!("failed to spawn Marrow worker thread: {error}"),
            );
            ExitCode::FAILURE
        }
    }
}

/// Print a typed `code: message` line to standard error. The thin CLI renders
/// only text; structured output returns with the commands that need it.
pub(crate) fn report_simple_error(code: Code, message: &str) {
    eprintln!(
        "{}",
        term_style::code_message(Stream::Stderr, code, message)
    );
}

/// The one stderr sentence naming an exhausted fixed compiler bound, in the exhausted
/// bound's own words. `check`, `image`, and `client` all report a resource limit with
/// this sentence. `run` and `test` report the same bound as an operational record whose
/// text projection is the description alone.
pub(crate) fn resource_limit_message(description: &str) -> String {
    format!("the compiler reached a fixed resource limit: {description}")
}

pub(crate) fn report_io_error(file: &str, error: &std::io::Error) {
    report_simple_error(Code::IoRead, &format!("failed to read {file}: {error}"));
}

/// Report a source file's parse diagnostics on standard error, each in the one
/// diagnostic form, with its help line when it carries one. The sole caller invokes
/// this only for source with parse errors, so there is no success arm.
pub(crate) fn report_parse(file: &str, diagnostics: &[marrow_syntax::Diagnostic]) {
    let palette = Palette::for_stream(Stream::Stderr);
    for diagnostic in diagnostics {
        eprintln!(
            "{}",
            palette.diagnostic(
                file,
                diagnostic.span.line,
                diagnostic.span.column,
                diagnostic.code,
                &diagnostic.message,
            )
        );
        if let Some(help) = &diagnostic.help {
            eprintln!("{} {help}", palette.paint(Style::Code, "help:"));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::ExitCode;

    use super::{Command, run_worker_thread};

    #[test]
    fn every_command_parses_from_its_own_name() {
        for command in Command::ALL {
            assert_eq!(Command::parse(command.name()), Some(command));
        }
        assert_eq!(Command::parse("lsp"), None);
    }

    #[test]
    fn worker_thread_spawn_error_returns_failure() {
        let result = run_worker_thread(Err(std::io::ErrorKind::WouldBlock.into()));

        assert_eq!(result, ExitCode::FAILURE);
    }

    #[test]
    fn worker_thread_returns_worker_exit_code() {
        let result = run_worker_thread(Ok(std::thread::spawn(|| ExitCode::from(7))));

        assert_eq!(result, ExitCode::from(7));
    }
}
