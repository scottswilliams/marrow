//! The one production path that writes the accepted catalog: freezing a
//! project's baseline durable identity and the atomic catalog writer that
//! `evolve apply` also advances through.

use std::io::Write;
use std::path::{Path, PathBuf};

use marrow_project::{DiscoverError, ProjectConfig};

use crate::CheckReport;
use crate::driver::check_project;
use crate::program::CheckedProgram;

/// Writing the catalog file failed, or the project could not be re-discovered after
/// the write. The caller surfaces the path and the underlying cause.
#[derive(Debug)]
pub enum CommitIdentityError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    Discover(DiscoverError),
}

/// Establish a project's baseline durable identity: write its first catalog proposal
/// to the project's catalog file and re-check the project against it. Returns
/// `Ok(None)` when there is nothing to establish — the project already has an accepted
/// catalog, or proposes none — so a project past its baseline never churns the file.
///
/// This is the one production path that writes the catalog, called by the authorized
/// state-establishing flows (running the program and `evolve apply`) once the source
/// checks clean; `check` never does, so it stays read-only.
///
/// It commits only the baseline. Once a catalog is accepted, every later change to
/// durable identity is an evolution stamped into the store under `evolve apply`'s
/// witness transaction, never silently advanced here. Auto-writing an evolution
/// proposal would reserve retired entries before the witness consumed them, dropping
/// the very entries a retire relies on.
pub fn commit_pending_identity(
    project_root: &Path,
    config: &ProjectConfig,
    program: &CheckedProgram,
) -> Result<Option<(CheckReport, CheckedProgram)>, CommitIdentityError> {
    if program.catalog.accepted_epoch.is_some() {
        return Ok(None);
    }
    let Some(proposal) = &program.catalog.proposal else {
        return Ok(None);
    };
    // A project with no durable surface — a plain script, no resources, stores, or
    // enums — has no identity to freeze. Writing an empty baseline catalog would be
    // pure noise, so leave the project without one.
    if proposal.entries.is_empty() {
        return Ok(None);
    }
    write_accepted_catalog(project_root, config, proposal)?;
    check_project(project_root, config)
        .map(Some)
        .map_err(CommitIdentityError::Discover)
}

/// Write `catalog` to the project's accepted-catalog file, creating its parent
/// directory. This is the single production catalog writer: [`commit_pending_identity`]
/// freezes a baseline through it, and an authorized `evolve apply` advances the
/// accepted file to the activated proposal through it once the store transaction has
/// committed. The byte form is the same pretty JSON both the baseline and an evolution
/// proposal already serialize to.
///
/// The accepted catalog is the project's durable ABI: every binding's stable identity is
/// resolved against it, so a torn write would brick the project. The write is therefore
/// all-or-nothing. The bytes land in a temp file in the same directory and are flushed to
/// disk, then an atomic rename swaps the complete file over the target so a reader sees
/// either the old catalog or the whole new one, never a prefix. The parent directory is
/// flushed last so the rename itself survives a crash. A failure before the rename leaves
/// the prior catalog intact and removes the temp file.
pub fn write_accepted_catalog(
    project_root: &Path,
    config: &ProjectConfig,
    catalog: &marrow_catalog::CatalogMetadata,
) -> Result<(), CommitIdentityError> {
    let path = project_root.join(&config.accepted_catalog);
    let parent = path.parent().unwrap_or(project_root);
    std::fs::create_dir_all(parent).map_err(|error| CommitIdentityError::Io {
        path: parent.to_path_buf(),
        error,
    })?;

    // The temp name must be unique per write so two concurrent writers never rename
    // a half-written file over each other's. The process-wide counter separates
    // calls within this process; the pid separates processes sharing the project.
    static TEMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = TEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut temp = path.clone();
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(format!(".tmp.{}.{seq}", std::process::id()));
    temp.set_file_name(name);

    let io = |path: &Path, error| CommitIdentityError::Io {
        path: path.to_path_buf(),
        error,
    };
    let write_temp = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(catalog.to_json_pretty().as_bytes())?;
        file.sync_all()
    };
    if let Err(error) = write_temp() {
        let _ = std::fs::remove_file(&temp);
        return Err(io(&temp, error));
    }
    if let Err(error) = std::fs::rename(&temp, &path) {
        let _ = std::fs::remove_file(&temp);
        return Err(io(&path, error));
    }
    // Flushing the directory persists the rename. A failure here is non-fatal: the
    // bytes and the rename are already durable, and the next write re-establishes
    // the entry, so the catalog is never left torn.
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}
