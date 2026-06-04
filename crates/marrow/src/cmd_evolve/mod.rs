//! `marrow evolve`: preview and apply source-native data evolution.

use std::process::ExitCode;

use marrow_check::evolution::preview;
use marrow_run::evolution::{ApplyError, FenceError, apply, verify_activation_completion};

use crate::{
    CheckFormat, commit_pending_identity, load_checked_project_with_format, report_simple_error,
    write_accepted_catalog,
};

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

pub(crate) fn check_data(dir: &str, format: CheckFormat) -> ExitCode {
    let Ok((config, program)) = load_checked_project_with_format(dir, format) else {
        return ExitCode::FAILURE;
    };
    let Ok(store) = store::preview_store(dir, &config, format) else {
        return ExitCode::FAILURE;
    };
    match preview(&program, &store) {
        Ok((witness, _diagnostics)) if witness.is_activatable() => {
            render::data_check_ok(dir, &witness, format);
            ExitCode::SUCCESS
        }
        Ok((witness, diagnostics)) => {
            render::blocked(&witness, &diagnostics, format);
            ExitCode::FAILURE
        }
        Err(error) => {
            report_simple_error(error.code(), &error.to_string(), format);
            ExitCode::FAILURE
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
    match preview(&program, &store) {
        Ok((witness, diagnostics)) => {
            render::preview(&witness, &diagnostics, input.format);
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
    // Applying an evolution is an authorized state-establishing flow, so a clean
    // source with pending durable identity has it frozen here before the witness is
    // built. This is a separate transactional step from consuming the preview witness:
    // once the catalog is committed, preview and apply run against the accepted
    // identity exactly as they would for an already-accepted project.
    let program = match commit_pending_identity(&input.dir, &config, program) {
        Ok(program) => program,
        Err(code) => return code,
    };
    let Ok(store) = store::apply_store(&input.dir, &config, input.format) else {
        return ExitCode::FAILURE;
    };
    // The store transaction (data plus epoch stamp) and the accepted-catalog file are
    // advanced as two steps, with the file written last. A crash between them leaves the
    // store at the activated epoch while the file still records the prior one. That
    // window is recoverable: re-running apply finds the store already at the proposal
    // epoch and completes by writing the file alone, doing no data re-apply and no
    // second stamp. Detecting it before the fence is essential, because the fence reads
    // the behind-by-one file as its accepted epoch and would reject the store as
    // evolved.
    match resume_completion(&input.dir, &config, &program, &store, input.format) {
        Ok(Some(code)) => return code,
        Ok(None) => {}
        Err(code) => return code,
    }
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
            // Advance the accepted-catalog file to the activated proposal as the final
            // step, after the store transaction has committed. A proposal of `None` is a
            // change that does not touch durable identity (a pure backfill), so the file
            // already matches and is left untouched.
            if let Some(proposal) = &program.catalog.proposal
                && let Err(code) = write_accepted_catalog(&input.dir, &config, proposal)
            {
                return code;
            }
            render::apply_success(&outcome, input.format);
            ExitCode::SUCCESS
        }
        Err(ApplyError::NotActivatable) => {
            render::blocked(&witness, &diagnostics, input.format);
            ExitCode::FAILURE
        }
        Err(error) => {
            render::apply_error(error, input.format);
            ExitCode::FAILURE
        }
    }
}

/// Complete a half-applied evolution whose store reached the proposal epoch before the
/// accepted-catalog file was written. Returns `Ok(Some(code))` when this apply was a
/// resume that finished by writing the file alone, `Ok(None)` when there is no resume to
/// perform, and `Err(code)` when reading the store or writing the file failed.
///
/// The resume signature is exact: the store is stamped at the proposal epoch while the
/// accepted file the program loaded is still one epoch behind. Activating the data is
/// already done, so the only remaining work is to bring the file forward.
fn resume_completion(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    program: &marrow_check::CheckedProgram,
    store: &marrow_store::tree::TreeStore,
    format: CheckFormat,
) -> Result<Option<ExitCode>, ExitCode> {
    let Some(proposal) = &program.catalog.proposal else {
        return Ok(None);
    };
    let store_epoch = match store.read_catalog_epoch() {
        Ok(epoch) => epoch,
        Err(error) => {
            report_simple_error(error.code(), &error.to_string(), format);
            return Err(ExitCode::FAILURE);
        }
    };
    if store_epoch != Some(proposal.epoch) || program.catalog.accepted_epoch >= Some(proposal.epoch)
    {
        return Ok(None);
    }
    // The epoch signature alone cannot prove the source still describes the evolution
    // the store committed: a divergent edit during the crash window can propose the
    // same next epoch. The completion verifier below recomputes the current witness
    // from source plus the accepted catalog and compares its digest/effects against
    // this stamped commit before the file can publish.
    let commit = match store.read_commit_metadata() {
        Ok(commit) => commit,
        Err(error) => {
            report_simple_error(error.code(), &error.to_string(), format);
            return Err(ExitCode::FAILURE);
        }
    };
    let Some(commit) = commit else {
        report_resume_drift(format);
        return Err(ExitCode::FAILURE);
    };
    if commit.activation_proposal_catalog_digest.as_deref() != Some(proposal.digest.as_str()) {
        report_resume_drift(format);
        return Err(ExitCode::FAILURE);
    }
    if verify_activation_completion(program, store, &commit).is_err() {
        report_resume_drift(format);
        return Err(ExitCode::FAILURE);
    }
    write_accepted_catalog(dir, config, proposal)?;
    render::apply_resumed(proposal.epoch, format);
    Ok(Some(ExitCode::SUCCESS))
}

fn report_resume_drift(format: CheckFormat) {
    let drift = FenceError::SchemaDrift;
    report_simple_error(drift.code(), &drift.message(), format);
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
