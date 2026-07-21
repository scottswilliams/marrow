use marrow_codes::Code;
use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

use crate::term_style::{Stream, Style};

mod cmd_check;
mod cmd_client;
mod cmd_fmt;
mod cmd_init;
mod cmd_lsp;
mod cmd_run;
mod cmd_test;
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
  marrow run <export> [--format jsonl] [-- <args>...]
  marrow test [--format text|jsonl] [--filter <substring>]
  marrow client typescript [--out <dir>]
  marrow lsp
  marrow --version
  marrow --help

This is the beta line's thin CLI. `init` creates a new project (a manifest and a
contained src tree). `fmt` formats every captured source file in a project
directory, or one Marrow source file, through the retained formatter. `check`
captures and checks a project, reporting each diagnostic with its span and, when
clean, each exported function's durable access demand in source spelling. `run`
compiles the project at the working directory, verifies the program image, and
runs an exported function. `test` discovers `test \"name\"` declarations, runs
each storeless through the verified image, and reports pass/fail/error. `client
typescript` compiles and verifies the project, then emits the generated strict
TypeScript client and the pinned Node supervision module. `lsp` runs the in-tree
language server over stdio, serving diagnostics, formatting, hover, and definition
to an editor from the compiler's published analysis facts. The data, doctor,
evolve, serve, backup, and restore commands are being refounded and return
through their later lanes; invoking one reports cli.command_unsupported.
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
    let Some(command) = command.to_str() else {
        eprintln!("unknown command: {}", command.to_string_lossy());
        eprintln!("run `marrow --help` for available commands");
        return ExitCode::from(2);
    };
    let Some(rest) = utf8_args(rest) else {
        report_simple_error(
            Code::ConfigInvalid.as_str(),
            "command arguments must be valid UTF-8",
        );
        return ExitCode::FAILURE;
    };
    dispatch(command, &rest)
}

fn utf8_args(args: &[OsString]) -> Option<Vec<String>> {
    args.iter()
        .map(|arg| arg.to_str().map(str::to_string))
        .collect()
}

/// The command names whose owning capability is being refounded and returns
/// through a later lane. Recognizing them keeps the not-yet-supported response
/// distinct from an unknown-command usage error.
const REFOUNDING_COMMANDS: &[&str] = &["data", "doctor", "evolve", "serve", "backup", "restore"];

fn dispatch(command: &str, rest: &[String]) -> ExitCode {
    match command {
        "check" => cmd_check::check(rest),
        "fmt" => cmd_fmt::fmt(rest),
        "init" => cmd_init::init(rest),
        "run" => cmd_run::run(rest),
        "test" => cmd_test::test(rest),
        "client" => cmd_client::client(rest),
        "lsp" => cmd_lsp::lsp(rest),
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
        other if REFOUNDING_COMMANDS.contains(&other) => not_yet_supported(other),
        other => {
            eprintln!("unknown command: {other}");
            eprintln!("run `marrow --help` for available commands");
            ExitCode::from(2)
        }
    }
}

/// Report a recognized-but-refounding command as not yet available on this beta
/// line. A typed `cli.command_unsupported` response — not a silent success and
/// not a usage error — so a script that runs a not-yet-refounded command sees a
/// stable code rather than mistaking absence for success.
fn not_yet_supported(command: &str) -> ExitCode {
    report_simple_error(
        Code::CliCommandUnsupported.as_str(),
        &format!(
            "`marrow {command}` is not available on this beta line yet; it returns through a later lane"
        ),
    );
    ExitCode::FAILURE
}

/// The stack the parse/format pipeline runs on. 256 MiB comfortably holds the
/// recursion the typed parser limit permits — 256 nested parser frames — with wide
/// margin, so the limit always trips before the stack does.
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
                Code::IoThread.as_str(),
                &format!("failed to spawn Marrow worker thread: {error}"),
            );
            ExitCode::FAILURE
        }
    }
}

/// Print a typed `code: message` line to standard error. The thin CLI renders
/// only text; structured output returns with the commands that need it.
pub(crate) fn report_simple_error(code: &str, message: &str) {
    eprintln!(
        "{}",
        term_style::code_message(Stream::Stderr, code, message)
    );
}

pub(crate) fn report_io_error(file: &str, error: &std::io::Error) {
    report_simple_error(
        Code::IoRead.as_str(),
        &format!("failed to read {file}: {error}"),
    );
}

pub(crate) fn unknown_option(command: &str, value: &str) -> ExitCode {
    eprintln!("unknown {command} option: {value}; run marrow {command} --help for usage");
    ExitCode::from(2)
}

/// Record one positional `target` into `slot`, rejecting a second one.
/// `target_label` names what the command takes so the error reads naturally.
pub(crate) fn take_single_target(
    slot: &mut Option<String>,
    target: &str,
    command: &str,
    target_label: &str,
) -> Result<(), ExitCode> {
    if slot.replace(target.to_string()).is_some() {
        eprintln!("marrow {command} accepts one {target_label}");
        return Err(ExitCode::from(2));
    }
    Ok(())
}

/// Report a source file's parse diagnostics on standard error. The sole caller
/// invokes this only for source with parse errors, so there is no success arm.
pub(crate) fn report_parse(file: &str, diagnostics: &[marrow_syntax::Diagnostic]) {
    for diagnostic in diagnostics {
        eprintln!("{}", syntax_diagnostic_line(file, diagnostic));
        if let Some(help) = &diagnostic.help {
            eprintln!(
                "{} {help}",
                term_style::paint(Stream::Stderr, Style::Code, "help:")
            );
        }
    }
}

fn severity_style(severity: &str) -> Style {
    match severity {
        "warning" => Style::Warning,
        _ => Style::Error,
    }
}

fn syntax_diagnostic_line(file: &str, diagnostic: &marrow_syntax::Diagnostic) -> String {
    format!(
        "{}:{}:{}: {}: {}: {}",
        term_style::paint(Stream::Stderr, Style::Muted, file),
        diagnostic.span.line,
        diagnostic.span.column,
        term_style::paint(
            Stream::Stderr,
            severity_style(diagnostic.severity.as_str()),
            diagnostic.severity.as_str(),
        ),
        term_style::paint(Stream::Stderr, Style::Code, diagnostic.code),
        diagnostic.message
    )
}

#[cfg(test)]
mod tests {
    use std::process::ExitCode;

    use super::run_worker_thread;

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
