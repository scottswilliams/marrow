//! `marrow evolve`: preview and apply source-native data evolution.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use marrow_check::evolution::{EvolutionWitness, Verdict, preview};
use marrow_run::evolution::{ApplyError, apply};

use crate::{
    load_checked_project_with_format, load_config_with_format, project_io_exit, report_simple_error,
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

fn preview_cmd(raw_args: &[String]) -> ExitCode {
    let input = match args::preview_args(raw_args) {
        Ok(input) => input,
        Err(args::ParseStop::Help) => return ExitCode::SUCCESS,
        Err(args::ParseStop::Usage) => return ExitCode::from(2),
    };
    let (program, store) = if let Some(backup) = &input.from_backup {
        let config = match load_config_with_format(&input.dir, input.format) {
            Ok(config) => config,
            Err(code) => return code,
        };
        match crate::cmd_restore::mount_backup_for_evolution_preview(
            &input.dir,
            &config,
            backup,
            input.format,
        ) {
            Ok(target) => target,
            Err(code) => return code,
        }
    } else {
        let Ok((config, program)) = load_checked_project_with_format(&input.dir, input.format)
        else {
            return ExitCode::FAILURE;
        };
        let Ok(store) = store::preview_store(&input.dir, &config, input.format) else {
            return ExitCode::FAILURE;
        };
        (program, store)
    };
    let labels = render::SourceLabels::from_program(&program);
    match preview(&program, &store) {
        Ok((witness, diagnostics)) => {
            render::preview(
                &witness,
                &diagnostics,
                &labels,
                input.format,
                input.scaffold,
            );
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
    if let Err(code) = guard_recovery_backup_path(&input, &config) {
        return code;
    }
    let Ok(store) = store::apply_store(&input.dir, &config, input.format) else {
        return ExitCode::FAILURE;
    };
    // Apply is an authorized state-establishing flow, so pending durable identity is
    // frozen into the store as its baseline before the witness is built; preview and apply
    // then run against the accepted identity exactly as for an already-accepted project.
    let program = if program.catalog.accepted_epoch.is_none() {
        match crate::establish_store_baseline(&input.dir, &config, &store, program, input.format) {
            Ok(program) => program,
            Err(code) => return code,
        }
    } else {
        program
    };
    let labels = render::SourceLabels::from_program(&program);
    let (witness, diagnostics) = match preview(&program, &store) {
        Ok(result) => result,
        Err(error) => {
            report_simple_error(error.code(), &error.to_string(), input.format);
            return ExitCode::FAILURE;
        }
    };
    let recovery = match prepare_recovery_point(&input, &witness, &program, &store) {
        Ok(recovery) => recovery,
        Err(code) => return code,
    };
    match apply(
        &witness,
        &program,
        &store,
        input.maintenance,
        input.approval.as_ref(),
    ) {
        Ok(outcome) => {
            // Apply is not done until the re-projected lock is committed: the store
            // transaction has published the activated catalog, and the source-tree lock
            // must converge to it before the command reports success.
            if let Err(code) =
                crate::reproject_committed_lock(&input.dir, &store, &program, input.format)
            {
                return code;
            }
            render::apply_success(&outcome, &recovery, input.format);
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

fn prepare_recovery_point(
    input: &args::ApplyArgs,
    witness: &EvolutionWitness,
    program: &marrow_check::CheckedProgram,
    store: &marrow_store::tree::TreeStore,
) -> Result<render::RecoveryPoint, ExitCode> {
    if let Some(path) = &input.backup {
        match crate::backup::create_backup_artifact(program, store, Path::new(path)) {
            Ok(_) => {
                return Ok(render::RecoveryPoint::Backup { path: path.clone() });
            }
            Err(error) => {
                report_simple_error(error.code(), &error.to_string(), input.format);
                return Err(ExitCode::FAILURE);
            }
        }
    }
    if input.no_backup {
        return Ok(render::RecoveryPoint::NoBackup);
    }
    if requires_recovery_point(witness) {
        render::requires_backup(input.format);
        return Err(ExitCode::FAILURE);
    }
    Ok(render::RecoveryPoint::None)
}

fn guard_recovery_backup_path(
    input: &args::ApplyArgs,
    config: &marrow_project::ProjectConfig,
) -> Result<(), ExitCode> {
    let Some(path) = input.backup.as_deref() else {
        return Ok(());
    };
    let backup_path = Path::new(path);
    let project_dir = Path::new(&input.dir);
    for project_root in [
        lexical_path(project_dir),
        resolved_or_lexical_path(project_dir),
    ] {
        let managed_paths = managed_recovery_backup_paths(&project_root, config)
            .map_err(|error| project_io_exit(&input.dir, error, input.format))?;
        for managed_path in managed_paths {
            if backup_conflicts_with_managed_path(backup_path, &managed_path.path) {
                report_simple_error(
                    "evolve.backup_path_managed",
                    &format!(
                        "evolve backup path must not target the {}: {}",
                        managed_path.label,
                        managed_path.path.display()
                    ),
                    input.format,
                );
                return Err(ExitCode::FAILURE);
            }
        }
    }
    Ok(())
}

struct ManagedRecoveryPath {
    label: &'static str,
    path: PathBuf,
}

fn managed_recovery_backup_paths(
    project_root: &Path,
    config: &marrow_project::ProjectConfig,
) -> Result<Vec<ManagedRecoveryPath>, marrow_check::ProjectIoError> {
    let mut paths = vec![
        ManagedRecoveryPath {
            label: "project config file",
            path: project_root.join("marrow.json"),
        },
        ManagedRecoveryPath {
            label: "committed lock",
            path: project_root.join(marrow_project::CATALOG_FILE_NAME),
        },
    ];
    for source_root in &config.source_roots {
        paths.push(ManagedRecoveryPath {
            label: "configured source root",
            path: project_root.join(source_root),
        });
    }
    for test_path in &config.tests {
        paths.push(ManagedRecoveryPath {
            label: "configured test path",
            path: project_root.join(test_path),
        });
    }
    if config.store.backend == marrow_project::StoreBackend::Native {
        if let Some(store_path) = marrow_check::native_store_path(project_root, config)? {
            paths.push(ManagedRecoveryPath {
                label: "configured native store file",
                path: store_path,
            });
        }
        if let Some(data_dir) = &config.store.data_dir {
            paths.push(ManagedRecoveryPath {
                label: "configured native data directory",
                path: project_root.join(data_dir),
            });
        }
    }
    Ok(paths)
}

fn backup_conflicts_with_managed_path(backup_path: &Path, managed_path: &Path) -> bool {
    path_contains_or_equals(lexical_path(backup_path), lexical_path(managed_path))
        || path_contains_or_equals(
            resolved_or_lexical_path(backup_path),
            resolved_or_lexical_path(managed_path),
        )
}

fn path_contains_or_equals(path: PathBuf, parent: PathBuf) -> bool {
    path == parent || path.starts_with(parent)
}

fn resolved_or_lexical_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    if let (Some(parent), Some(file_name)) = (path.parent(), path.file_name())
        && let Ok(canonical_parent) = fs::canonicalize(parent)
    {
        return normalize_lexical_path(canonical_parent.join(file_name));
    }
    lexical_path(path)
}

fn lexical_path(path: &Path) -> PathBuf {
    normalize_lexical_path(if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    })
}

fn normalize_lexical_path(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn requires_recovery_point(witness: &EvolutionWitness) -> bool {
    witness.verdicts.iter().any(|obligation| {
        matches!(
            obligation.verdict,
            Verdict::DestructiveDecisionRequired { .. }
        )
    })
}

fn print_help() {
    print!(
        "\
Usage:
  marrow evolve preview [--from-backup <artifact>] [--scaffold] [--format text|json|jsonl] <projectdir>
  marrow evolve apply [--maintenance] [--approve-retire <catalog-id>:<count>] \
    [--backup <path> | --no-backup] \
    [--format text|json|jsonl] <projectdir>

Preview attached-data evolution, or apply the exact current preview witness.
"
    );
}
