use std::process::ExitCode;

const HELP: &str = "\
Marrow

Usage:
  marrow --version
  marrow --help

Marrow is starting from the reference docs. Language commands will land as the
native .mw parser, checker, runtime, and storage kernel are implemented.
";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None | Some("--help" | "-h" | "help") => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some("--version" | "-V" | "version") => {
            println!("marrow {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("unknown command: {command}");
            eprintln!("run `marrow --help` for available commands");
            ExitCode::from(2)
        }
    }
}
