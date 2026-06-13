//! `marrow evolve`: preview and apply source-native data evolution.

use std::process::ExitCode;

use marrow_check::evolution::preview;
use marrow_run::evolution::{ApplyError, apply};

use crate::{load_checked_project_with_format, report_simple_error};

mod args;
mod render;
mod store;

pub(crate) fn evolve(args: &[String]) -> ExitCode {
    let Some((command, rest)) = args.split_first() else {
        print_help();
        return ExitCode::from(2);
    };
    match command.as_str() {
        "preview" => preview_cmd(rest),
        "apply" => apply_cmd(rest),
        "--help" | "-h" | "help" => {
            print_help();
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("unknown evolve subcommand: {other}");
            eprintln!("available evolve subcommands: preview, apply");
            ExitCode::from(2)
        }
    }
}

fn preview_cmd(raw_args: &[String]) -> ExitCode {
    let input = match args::preview_args(raw_args) {
        Ok(input) => input,
        Err(args::ParseStop::Help) => return ExitCode::SUCCESS,
        Err(args::ParseStop::Usage) => return ExitCode::from(2),
    };
    let Ok((config, program)) = load_checked_project_with_format(&input.dir, input.format) else {
        return ExitCode::FAILURE;
    };
    let Ok(store) = store::preview_store(&input.dir, &config, input.format) else {
        return ExitCode::FAILURE;
    };
    let labels = render::SourceLabels::from_program(&program);
    match preview(&program, &store) {
        Ok((witness, diagnostics)) => {
            render::preview(&witness, &diagnostics, &labels, input.format);
            if witness.is_activatable() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            report_simple_error(error.code(), &error.to_string(), input.format);
            ExitCode::FAILURE
        }
    }
}

fn apply_cmd(raw_args: &[String]) -> ExitCode {
    let input = match args::apply_args(raw_args) {
        Ok(input) => input,
        Err(args::ParseStop::Help) => return ExitCode::SUCCESS,
        Err(args::ParseStop::Usage) => return ExitCode::from(2),
    };
    let Ok((config, program)) = load_checked_project_with_format(&input.dir, input.format) else {
        return ExitCode::FAILURE;
    };
    let Ok(store) = store::apply_store(&input.dir, &config, input.format) else {
        return ExitCode::FAILURE;
    };
    // Apply is an authorized state-establishing flow, so pending durable identity is
    // frozen into the store as its baseline before the witness is built; preview and apply
    // then run against the accepted identity exactly as for an already-accepted project.
    let program =
        match crate::establish_store_baseline(&input.dir, &config, &store, program, input.format) {
            Ok(program) => program,
            Err(code) => return code,
        };
    let labels = render::SourceLabels::from_program(&program);
    let (witness, diagnostics) = match preview(&program, &store) {
        Ok(result) => result,
        Err(error) => {
            report_simple_error(error.code(), &error.to_string(), input.format);
            return ExitCode::FAILURE;
        }
    };
    match apply(
        &witness,
        &program,
        &store,
        input.maintenance,
        input.approval.as_ref(),
    ) {
        Ok(outcome) => {
            if let Err(code) =
                crate::render_accepted_catalog_file_from_store(&input.dir, &store, input.format)
            {
                return code;
            }
            render::apply_success(&outcome, input.format);
            ExitCode::SUCCESS
        }
        Err(ApplyError::NotActivatable) => {
            render::blocked(&witness, &diagnostics, &labels, input.format);
            ExitCode::FAILURE
        }
        Err(error) => {
            render::apply_error(error, &labels, input.format);
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    print!(
        "\
Usage:
  marrow evolve preview [--format text|json|jsonl] <projectdir>
  marrow evolve apply [--maintenance] [--approve-retire <catalog-id>:<count>] \
    [--format text|json|jsonl] <projectdir>

Preview attached-data evolution, or apply the exact current preview witness.
"
    );
}
