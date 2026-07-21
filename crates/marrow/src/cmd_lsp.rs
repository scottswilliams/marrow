//! `marrow lsp`: run the in-tree language server over stdio.
//!
//! The command takes no arguments (a `--help` flag prints usage). It hands control to
//! [`marrow_lsp::serve`], which owns the whole protocol lifecycle over stdin/stdout and
//! returns a process exit code.

use std::process::ExitCode;

const HELP: &str = "\
Usage:
  marrow lsp

Run the Marrow language server over stdio (JSON-RPC 2.0 with LSP framing). The
server captures and analyzes the project at the client-selected workspace root and
serves diagnostics, formatting, hover, and definition over the compiler's published
analysis facts. It takes no arguments and is normally launched by an editor, not run
by hand.
";

pub(crate) fn lsp(args: &[String]) -> ExitCode {
    match args.first() {
        None => ExitCode::from(marrow_lsp::serve()),
        Some(arg) if arg == "--help" || arg == "-h" => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some(other) => crate::unknown_option("lsp", other),
    }
}
