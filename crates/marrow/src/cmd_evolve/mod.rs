//! `marrow evolve`: preview and apply source-native data evolution.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use marrow_check::evolution::{EvolutionWitness, Verdict, preview};
use marrow_run::evolution::{ApplyError, Approval, FenceError, apply};
use marrow_store::cell::CatalogId;

use crate::{
    CheckFormat, load_checked_project_with_format, load_config_with_format, project_io_exit,
    report_simple_error,
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
                &input.dir,
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
    // The store is created or baselined below, so the committed-root witness runs first: a store
    // that lost the roots its lock recorded fails closed here rather than being re-baselined over
    // the loss, the same verdict the read-only inspection family reaches.
    if let Err(code) = guard_committed_lock_roots(&input.dir, &config, input.format) {
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
    let approval = match resolve_approval(&input.retires, &program, input.format) {
        Ok(approval) => approval,
        Err(code) => return code,
    };
    let (witness, diagnostics) = match preview(&program, &store) {
        Ok(result) => result,
        Err(error) => {
            report_simple_error(error.code(), &error.to_string(), input.format);
            return ExitCode::FAILURE;
        }
    };
    if let Err(code) = guard_store_behind_committed_lock(&input.dir, &witness, input.format) {
        return code;
    }
    let recovery = match prepare_recovery_point(&input, &witness, &program, &store) {
        Ok(recovery) => recovery,
        Err(code) => return code,
    };
    match apply(
        &witness,
        &program,
        &store,
        input.maintenance,
        approval.as_ref(),
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
            // The activated catalog can change the surface ABI, so refresh the declared client in
            // lockstep with the re-projected lock.
            if let Err(code) =
                crate::sync_declared_client(&input.dir, &config, &program, input.format)
            {
                return code;
            }
            render::apply_success(&outcome, &recovery, input.format);
            ExitCode::SUCCESS
        }
        Err(ApplyError::NotActivatable) => {
            render::blocked(&input.dir, &witness, &diagnostics, &labels, input.format);
            ExitCode::FAILURE
        }
        Err(ApplyError::ApprovalMismatch) => {
            render::approval_mismatch(&witness, &labels, input.format);
            ExitCode::FAILURE
        }
        Err(error) => {
            render::apply_error(error, &labels, input.format);
            ExitCode::FAILURE
        }
    }
}

/// Resolve the `--approve-retire` targets against the checked program into one approval, accepting
/// the resource-qualified field path (`Book.pages`), the module-qualified catalog path, or the
/// internal catalog id. The field path is the everyday form the approval message and scaffold teach;
/// the other two still resolve so scripts that already pass them keep working. A target that matches
/// none of these is a usage error naming the unresolved target. One approval covers a multi-id
/// destructive evolution, and admission still matches each id and count against the witness exactly.
fn resolve_approval(
    retires: &[args::RetireSpec],
    program: &marrow_check::CheckedProgram,
    format: CheckFormat,
) -> Result<Option<Approval>, ExitCode> {
    if retires.is_empty() {
        return Ok(None);
    }
    let mut resolved = Vec::with_capacity(retires.len());
    for spec in retires {
        let id = resolve_retire_target(&spec.target, program).ok_or_else(|| {
            report_simple_error(
                "evolve.approval_target_unknown",
                &format!(
                    "--approve-retire target `{}` is not a field path or catalog id in this project; \
                     run `marrow evolve preview <projectdir>` to see the exact path to approve",
                    spec.target
                ),
                format,
            );
            ExitCode::from(2)
        })?;
        resolved.push((id, spec.populated));
    }
    Ok(Some(Approval { retires: resolved }))
}

/// The catalog id a `--approve-retire` target names. The everyday form is the resource-qualified
/// field path the reference and scaffold teach (`Book.pages`); the module-qualified catalog path
/// (`books::Book::pages`) and the internal stable id both still resolve so existing scripts keep
/// working. The resource-qualified form is matched against each entry's spelling below its owning
/// module, which is the same spelling the scaffold prints, so the two never diverge.
fn resolve_retire_target(
    target: &str,
    program: &marrow_check::CheckedProgram,
) -> Option<CatalogId> {
    let entries = &program.catalog.accepted_entries;
    let id = entries
        .iter()
        .find(|entry| resource_qualified_path(program, &entry.path).as_deref() == Some(target))
        .or_else(|| entries.iter().find(|entry| entry.path == target))
        .or_else(|| entries.iter().find(|entry| entry.stable_id == target))?
        .stable_id
        .clone();
    CatalogId::new(id).ok()
}

/// A catalog entry's resource-qualified surface spelling (`Book.pages`): the dot-joined segments
/// below its owning module, matching the form the reference documents and the scaffold prints. An
/// entry whose path no module owns has no such spelling.
fn resource_qualified_path(program: &marrow_check::CheckedProgram, path: &str) -> Option<String> {
    let (_, local) = render::owned_path(program, path)?;
    Some(local.join("."))
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

/// Refuse a desynced apply: a local store behind a committed lock whose epoch high-water this
/// single activation cannot reach. The committed lock records the accepted epoch a teammate already
/// activated and committed; when its high-water exceeds the epoch this apply would land the store
/// at, the intervening activations (and any retire that minted a tombstone) cannot be reconstructed
/// locally, and minting a fresh activation below the high-water would both collide with the
/// teammate's already-committed identity and force the lock projection to either regress or skip
/// reserved history. The operator must catch the local store up first (re-run to seed from the lock,
/// or restore the store) before applying. An exact catch-up to the lock's high-water is allowed, so
/// the single-step advance a stale checkout performs against an ahead committed catalog still works.
fn guard_store_behind_committed_lock(
    dir: &str,
    witness: &EvolutionWitness,
    format: CheckFormat,
) -> Result<(), ExitCode> {
    let Some(lock) = crate::read_committed_lock(dir, format)? else {
        return Ok(());
    };
    let (_, target_epoch) = witness.epoch_range();
    if lock.epoch_high_water > target_epoch {
        // Share the typed `run.store_behind` code with the run-path fence so the code stays
        // single-owner, but the remedy is the apply-desync one: applying here would land the
        // store below the committed high-water, so re-running apply would only fail closed
        // again. The operator must reconcile the local store with the team's up-to-date store
        // first, never re-run the command that just refused.
        let code = FenceError::StoreBehind {
            stored: target_epoch,
            accepted: lock.epoch_high_water,
        }
        .code();
        report_simple_error(
            code,
            &apply_desync_remedy(target_epoch, lock.epoch_high_water),
            format,
        );
        return Err(ExitCode::FAILURE);
    }
    Ok(())
}

/// The remedy for a desynced apply: the local store is behind the committed lock by more than the
/// single catch-up step this activation can take, so applying would regress or erase the committed
/// lock. Unlike the run-path fence this never advises re-running the failing command; the operator
/// must first pull or rebuild the local store to match the committed lock's high-water.
fn apply_desync_remedy(target_epoch: u64, high_water: u64) -> String {
    format!(
        "applying would land the store at epoch {target_epoch}, below the committed lock's epoch \
         high-water {high_water}; a teammate has already activated and committed past this local \
         store, so this apply would regress or erase the committed lock. Reconcile the local store \
         with the team's up-to-date store first (pull or rebuild the store that matches the \
         committed lock), then re-check; do not re-run apply against this stale store."
    )
}

/// Fail apply closed when the store has lost the committed roots its `marrow.lock` records, before
/// `apply_store` would create or baseline a fresh store over the loss. The committed lock is the
/// independent witness to durable identity: a store presenting fewer of its active roots than the
/// lock recorded — rolled back, partially dropped, crashed mid-creation, or wholly deleted while
/// its lock survives — has lost durable identity and is `store.corruption`, not a clean store to
/// re-baseline. This is the same verdict the read-only inspection family reaches, routed through
/// the one race-aware witness owner so a writer mid-re-creating the store yields `store.locked`
/// rather than a false corruption. A genuine first apply records no active root in the lock, so
/// the witness never fires and the fresh baseline path runs.
///
/// A store committed at an earlier epoch than the lock's high-water is a legitimately-behind local
/// checkout — the very store apply exists to advance — so it carries every committed root but
/// fewer member entries than the ahead lock and is left to the apply, never condemned. A rolled
/// back, lost, or crash-mid-creation store carries no usable commit metadata, so it is not behind
/// and the witness fires.
fn guard_committed_lock_roots(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    format: CheckFormat,
) -> Result<(), ExitCode> {
    let Some(path) = marrow_check::native_store_path(Path::new(dir), config)
        .map_err(|error| project_io_exit(dir, error, format))?
    else {
        return Ok(());
    };
    let Some(lock) = crate::read_committed_lock(dir, format)? else {
        return Ok(());
    };
    if !lock.records_active_roots() {
        return Ok(());
    }
    let store = if marrow_check::tooling::store_path_is_absent(&path) {
        None
    } else {
        crate::open_store_for_inspection(dir, config, format)?
    };
    if let Some(store) = &store
        && store_is_behind_lock(store, &lock, format)?
    {
        return Ok(());
    }
    match crate::verify_lock_roots_or_race(store.as_ref(), Some(&path), Some(&lock)) {
        crate::LockRootVerdict::Clean => Ok(()),
        crate::LockRootVerdict::Lost(error) => {
            report_simple_error(error.code(), &error.to_string(), format);
            Err(ExitCode::FAILURE)
        }
    }
}

/// Whether a present store is a legitimately-behind local checkout: it carries committed metadata
/// at an epoch below the lock's high-water, the store-behind case apply advances. A store with no
/// commit metadata — rolled back, wiped, or crashed mid-creation — is not behind, so the lock-root
/// witness still condemns it.
fn store_is_behind_lock(
    store: &marrow_store::tree::TreeStore,
    lock: &marrow_catalog::CatalogLock,
    format: CheckFormat,
) -> Result<bool, ExitCode> {
    match store.read_commit_metadata() {
        Ok(Some(commit)) => Ok(lock.epoch_high_water > commit.catalog_epoch),
        Ok(None) => Ok(false),
        Err(error) => {
            report_simple_error(error.code(), &error.to_string(), format);
            Err(ExitCode::FAILURE)
        }
    }
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
  marrow evolve apply [--maintenance] [--approve-retire <field-path>:<count>] \
    [--backup <path> | --no-backup] \
    [--format text|json|jsonl] <projectdir>

Preview attached-data evolution, or apply the exact current preview witness.
"
    );
}
