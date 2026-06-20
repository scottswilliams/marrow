use std::process::ExitCode;

mod typescript;

const HELP: &str = "\
Usage:
  marrow surface client typescript <projectdir>
  marrow surface client --help

Generate descriptor-derived application-surface clients.
";

pub(crate) fn client(args: &[String]) -> ExitCode {
    let Some((command, rest)) = args.split_first() else {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    };
    match command.as_str() {
        "typescript" => typescript::typescript(rest),
        "--help" | "-h" | "help" => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("unknown surface client command: {other}");
            eprintln!("run `marrow surface client --help` for available commands");
            ExitCode::from(2)
        }
    }
}
