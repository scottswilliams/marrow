use std::process::ExitCode;

mod typescript;

const HELP: &str = "\
Usage:
  marrow client typescript [--cursor-token] [--out <path>] <projectdir>
  marrow client --help

Generate descriptor-derived application-surface clients.

  --cursor-token  Generate the remote cursor-token client profile.
  --out           Write the client to <path>; prints to stdout when omitted.
";

pub(crate) fn client(args: &[String]) -> ExitCode {
    let Some((command, rest)) = args.split_first() else {
        // A bare `marrow client` named no subcommand, so it ran nothing: exit 2 like every other
        // missing-subcommand usage error rather than passing a CI gate green.
        eprint!("{HELP}");
        return ExitCode::from(2);
    };
    match command.as_str() {
        "typescript" => typescript::typescript(rest),
        "--help" | "-h" | "help" => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("unknown client command: {other}");
            eprintln!("run `marrow client --help` for available commands");
            ExitCode::from(2)
        }
    }
}
