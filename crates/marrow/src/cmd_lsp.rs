//! `marrow lsp`: run the sibling language-server binary over stdio.
//!
//! The command takes no arguments (a `--help` flag prints usage). It launches only the
//! fixed `marrow-lsp` executable beside the current CLI, never a search-path candidate,
//! and inherits stdin/stdout so the server continues to own the whole protocol stream.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Stdio};

use marrow_codes::Code;

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
        None => launch_server(),
        Some(arg) if arg == "--help" || arg == "-h" => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some(other) => crate::unknown_option("lsp", other),
    }
}

fn launch_server() -> ExitCode {
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => return launch_error(&error.to_string()),
    };
    let Some(server) = sibling_server_path(&executable) else {
        return launch_error("the current executable has no parent directory");
    };
    match Command::new(&server)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
    {
        Ok(status) => child_exit_code(status),
        Err(error) => launch_error(&format!("{}: {error}", server.display())),
    }
}

fn sibling_server_path(executable: &Path) -> Option<PathBuf> {
    Some(
        executable
            .parent()?
            .join(format!("marrow-lsp{}", std::env::consts::EXE_SUFFIX)),
    )
}

fn child_exit_code(status: ExitStatus) -> ExitCode {
    status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .map_or(ExitCode::FAILURE, ExitCode::from)
}

fn launch_error(error: &str) -> ExitCode {
    crate::report_simple_error(
        Code::IoRead.as_str(),
        &format!("failed to launch the installed language server: {error}"),
    );
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::sibling_server_path;

    #[test]
    fn server_path_is_the_fixed_sibling_of_the_running_cli() {
        let executable =
            Path::new("installation").join(format!("marrow{}", std::env::consts::EXE_SUFFIX));
        assert_eq!(
            sibling_server_path(&executable),
            Some(
                Path::new("installation")
                    .join(format!("marrow-lsp{}", std::env::consts::EXE_SUFFIX))
            )
        );
    }
}
