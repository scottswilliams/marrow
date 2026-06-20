use std::process::ExitCode;

mod serve;

const HELP: &str = "\
Usage:
  marrow surface serve [--write] [--cors-origin <loopback-origin>] [--addr <loopback:port>] <projectdir>
  marrow surface --help

Expose descriptor-derived application-surface routes for local tooling.
";

pub(crate) fn surface(args: &[String]) -> ExitCode {
    let Some((command, rest)) = args.split_first() else {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    };
    match command.as_str() {
        "serve" => serve::serve(rest),
        "--help" | "-h" | "help" => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("unknown surface command: {other}");
            eprintln!("run `marrow surface --help` for available commands");
            ExitCode::from(2)
        }
    }
}
