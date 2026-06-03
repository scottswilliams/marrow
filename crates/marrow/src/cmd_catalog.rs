//! `marrow catalog`: preview or accept generated catalog metadata.

use std::path::Path;
use std::process::ExitCode;

use crate::{
    CheckFormat, load_checked_project_with_format, report_io_error, report_project,
    report_simple_error, write_json,
};

pub(crate) fn catalog(args: &[String]) -> ExitCode {
    let Some((command, rest)) = args.split_first() else {
        print_help();
        return ExitCode::from(2);
    };
    match command.as_str() {
        "preview" => preview(rest),
        "accept" => accept(rest),
        "--help" | "-h" | "help" => {
            print_help();
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("unknown catalog subcommand: {other}");
            eprintln!("run `marrow catalog --help` for available commands");
            ExitCode::from(2)
        }
    }
}

fn preview(args: &[String]) -> ExitCode {
    let input = match parse_args(args, "catalog preview") {
        Ok(input) => input,
        Err(ParseStop::Help) => return ExitCode::SUCCESS,
        Err(ParseStop::Usage) => return ExitCode::from(2),
    };
    let Ok((_config, program)) = load_checked_project_with_format(&input.dir, input.format) else {
        return ExitCode::FAILURE;
    };
    render_preview(&program, input.format);
    ExitCode::SUCCESS
}

fn accept(args: &[String]) -> ExitCode {
    let input = match parse_args(args, "catalog accept") {
        Ok(input) => input,
        Err(ParseStop::Help) => return ExitCode::SUCCESS,
        Err(ParseStop::Usage) => return ExitCode::from(2),
    };
    let Ok((config, program)) = load_checked_project_with_format(&input.dir, input.format) else {
        return ExitCode::FAILURE;
    };
    let Some(proposal) = program.catalog.proposal.clone() else {
        render_current(&program, input.format);
        return ExitCode::SUCCESS;
    };

    match marrow_check::accept_catalog_proposal(Path::new(&input.dir), &config, &program) {
        Ok(Some((report, _program))) if report.has_errors() => {
            report_project(&input.dir, &report, input.format);
            ExitCode::FAILURE
        }
        Ok(_) => {
            render_accepted(proposal.epoch, proposal.entries.len(), input.format);
            ExitCode::SUCCESS
        }
        Err(marrow_check::AcceptError::Io { path, error }) => {
            report_io_error(&path.display().to_string(), &error, input.format);
            ExitCode::FAILURE
        }
        Err(marrow_check::AcceptError::Discover(error)) => {
            report_simple_error(
                error.code,
                &format!("{}: {}", error.path.display(), error.message),
                input.format,
            );
            ExitCode::FAILURE
        }
    }
}

struct Parsed {
    format: CheckFormat,
    dir: String,
}

enum ParseStop {
    Help,
    Usage,
}

fn parse_args(args: &[String], command: &str) -> Result<Parsed, ParseStop> {
    let mut format = CheckFormat::Text;
    let mut saw_format = false;
    let mut dir = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--format" => {
                if saw_format {
                    eprintln!("duplicate --format");
                    return Err(ParseStop::Usage);
                }
                saw_format = true;
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --format");
                    return Err(ParseStop::Usage);
                };
                let Some(parsed) = CheckFormat::parse(value) else {
                    eprintln!("unknown {command} format: {value}");
                    return Err(ParseStop::Usage);
                };
                format = parsed;
            }
            "--help" | "-h" => {
                print_help();
                return Err(ParseStop::Help);
            }
            value if value.starts_with('-') => {
                eprintln!("unknown {command} option: {value}");
                return Err(ParseStop::Usage);
            }
            value => {
                if dir.replace(value.to_string()).is_some() {
                    eprintln!("{command} accepts one project directory");
                    return Err(ParseStop::Usage);
                }
            }
        }
        index += 1;
    }
    let Some(dir) = dir else {
        eprintln!("missing project directory");
        return Err(ParseStop::Usage);
    };
    Ok(Parsed { format, dir })
}

fn render_preview(program: &marrow_check::CheckedProgram, format: CheckFormat) {
    match (&program.catalog.proposal, format) {
        (Some(proposal), CheckFormat::Text) => {
            println!("catalog preview");
            println!("status: proposal");
            println!("proposal epoch: {}", proposal.epoch);
            println!("digest: {}", proposal.digest);
            println!("entries: {}", proposal.entries.len());
        }
        (None, CheckFormat::Text) => {
            println!("catalog preview");
            println!("status: current");
            println!(
                "accepted epoch: {}",
                program.catalog.accepted_epoch.unwrap_or(0)
            );
        }
        (Some(proposal), CheckFormat::Json) => write_json(serde_json::json!({
            "kind": "catalog_preview",
            "status": "proposal",
            "proposal_epoch": proposal.epoch,
            "digest": proposal.digest,
            "entries": proposal.entries.len(),
        })),
        (None, CheckFormat::Json) => write_json(serde_json::json!({
            "kind": "catalog_preview",
            "status": "current",
            "accepted_epoch": program.catalog.accepted_epoch.unwrap_or(0),
        })),
        (Some(proposal), CheckFormat::Jsonl) => write_json(serde_json::json!({
            "kind": "catalog_preview",
            "status": "proposal",
            "proposal_epoch": proposal.epoch,
            "digest": proposal.digest,
            "entries": proposal.entries.len(),
        })),
        (None, CheckFormat::Jsonl) => write_json(serde_json::json!({
            "kind": "catalog_preview",
            "status": "current",
            "accepted_epoch": program.catalog.accepted_epoch.unwrap_or(0),
        })),
    }
}

fn render_current(program: &marrow_check::CheckedProgram, format: CheckFormat) {
    match format {
        CheckFormat::Text => println!(
            "accepted catalog current at epoch {}",
            program.catalog.accepted_epoch.unwrap_or(0)
        ),
        CheckFormat::Json | CheckFormat::Jsonl => write_json(serde_json::json!({
            "kind": "catalog_accept",
            "status": "current",
            "accepted_epoch": program.catalog.accepted_epoch.unwrap_or(0),
        })),
    }
}

fn render_accepted(epoch: u64, entries: usize, format: CheckFormat) {
    match format {
        CheckFormat::Text => {
            println!("accepted catalog epoch {epoch}");
            println!("entries: {entries}");
        }
        CheckFormat::Json | CheckFormat::Jsonl => write_json(serde_json::json!({
            "kind": "catalog_accept",
            "status": "accepted",
            "accepted_epoch": epoch,
            "entries": entries,
        })),
    }
}

fn print_help() {
    print!(
        "\
Usage:
  marrow catalog preview [--format text|json|jsonl] <projectdir>
  marrow catalog accept [--format text|json|jsonl] <projectdir>

Preview the accepted catalog proposal, or write the exact current proposal to
the project's acceptedCatalog file.
"
    );
}
