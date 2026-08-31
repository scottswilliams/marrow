//! The standalone `marrow-lsp` command.

use std::ffi::OsStr;
use std::process::ExitCode;

const HELP: &str = "\
Usage:
  marrow-lsp

Run the Marrow language server over stdio (JSON-RPC 2.0 with LSP framing). The
server takes no arguments and is normally launched by an editor.
";

fn main() -> ExitCode {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [] => ExitCode::from(marrow_lsp::serve()),
        [arg] if arg == OsStr::new("--help") || arg == OsStr::new("-h") => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        [arg, ..] => {
            eprintln!(
                "unknown marrow-lsp option: {}; run marrow-lsp --help for usage",
                arg.to_string_lossy()
            );
            ExitCode::from(2)
        }
    }
}
