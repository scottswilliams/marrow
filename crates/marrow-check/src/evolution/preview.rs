//! Read-only evolution preview facts and discharge witnesses.
//!
//! `evolution_preview` is the analysis-API surface. It records the checked schema
//! fingerprints and, when given a backup archive, bounded backup cell evidence. It
//! does not open a live store or decide activation. `preview` is the older
//! live-store discharge entry point: it reads store metadata, runs discharge
//! obligations, and returns the witness plus diagnostics without mutating data.

use std::fmt;
use std::path::Path;

use marrow_store::StoreError;
use marrow_store::cell::{DataCellKind, DataPathSegment};
use marrow_store::tree::TreeStore;
use marrow_store::tree::{
    TREE_BACKUP_MAX_CATALOG_SECTION_BYTES, TREE_BACKUP_MAX_CELL_BYTES,
    TREE_BACKUP_MAX_MANIFEST_BYTES, TreeBackupArchiveReadError, TreeBackupCellBuf,
    TreeBackupCellReadError, read_tree_backup_archive_chunk, read_tree_backup_archive_header,
};

use super::discharge::{RepairDiagnostic, discharge};
use super::witness::{CatalogFingerprint, EvolutionWitness};
use crate::analysis::AnalysisSnapshot;
use crate::program::{CheckedProgram, ProgramCatalog};

const MAX_BACKUP_CATALOG_ID_SAMPLES: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessFactSet {
    pub source_digest: String,
    pub evolution_digest: String,
    pub accepted_catalog: CatalogFingerprint,
    pub proposal_catalog: Option<CatalogFingerprint>,
    pub backup: Option<BackupWitnessFactSet>,
    pub live_store: LiveStorePreviewStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupWitnessFactSet {
    pub cell_count: u64,
    pub sample_catalog_ids: Vec<String>,
    pub samples_truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveStorePreviewStatus {
    Deferred,
}

#[derive(Debug)]
pub enum EvolutionPreviewError {
    Io(String),
    BackupFormat(String),
    BackupCell(TreeBackupCellReadError),
}

impl fmt::Display for EvolutionPreviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) => write!(f, "preview I/O failed: {message}"),
            Self::BackupFormat(message) => write!(f, "backup format is invalid: {message}"),
            Self::BackupCell(error) => write!(f, "backup cell stream is invalid: {error}"),
        }
    }
}

impl std::error::Error for EvolutionPreviewError {}

impl From<std::io::Error> for EvolutionPreviewError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<TreeBackupCellReadError> for EvolutionPreviewError {
    fn from(error: TreeBackupCellReadError) -> Self {
        Self::BackupCell(error)
    }
}

impl From<TreeBackupArchiveReadError> for EvolutionPreviewError {
    fn from(error: TreeBackupArchiveReadError) -> Self {
        Self::BackupFormat(error.to_string())
    }
}

/// The accepted and proposal catalog fingerprints a program's catalog carries. The
/// accepted fingerprint folds an absent epoch or digest to the empty baseline; the
/// proposal fingerprint is present only when the program emitted a proposal.
fn catalog_fingerprints(
    catalog: &ProgramCatalog,
) -> (CatalogFingerprint, Option<CatalogFingerprint>) {
    let accepted = CatalogFingerprint {
        epoch: catalog.accepted_epoch.unwrap_or(0),
        digest: catalog.accepted_digest.clone().unwrap_or_default(),
    };
    let proposal = catalog
        .proposal
        .as_ref()
        .map(|proposal| CatalogFingerprint {
            epoch: proposal.epoch,
            digest: proposal.digest.clone(),
        });
    (accepted, proposal)
}

pub fn evolution_preview(
    snapshot: &AnalysisSnapshot,
    backup: Option<&Path>,
) -> Result<WitnessFactSet, EvolutionPreviewError> {
    let (source_digest, evolution_digest) =
        crate::catalog::source_and_evolution_digests(&snapshot.program);
    let backup = backup.map(read_backup_witness_facts).transpose()?;
    let (accepted_catalog, proposal_catalog) = catalog_fingerprints(&snapshot.program.catalog);
    Ok(WitnessFactSet {
        source_digest,
        evolution_digest,
        accepted_catalog,
        proposal_catalog,
        backup,
        live_store: LiveStorePreviewStatus::Deferred,
    })
}

/// Discharge every obligation against `store` and assemble the evolution witness.
/// Strictly read-only. The witness composes the source and catalog fingerprints
/// with the store's engine profile, layout epoch, and latest commit id; the
/// diagnostics are the discharge's fail-closed messages.
pub fn preview(
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<(EvolutionWitness, Vec<RepairDiagnostic>), StoreError> {
    let discharge = discharge(program, store)?;

    let commit = store.read_commit_metadata()?;
    let store_source_digest = commit.as_ref().map(|commit| commit.source_digest.clone());
    let engine_profile_digest = commit.as_ref().map(|commit| commit.engine_profile_digest);
    let layout_epoch = commit.as_ref().map(|commit| commit.layout_epoch);
    let store_catalog = store
        .read_catalog_snapshot()?
        .map(|snapshot| CatalogFingerprint {
            epoch: snapshot.epoch,
            digest: snapshot.digest,
        });
    let (source_digest, evolution_digest) = crate::catalog::source_and_evolution_digests(program);
    let (accepted_catalog, proposal_catalog) = catalog_fingerprints(&program.catalog);
    let witness = EvolutionWitness {
        source_digest,
        evolution_digest,
        accepted_catalog,
        proposal_catalog,
        store_catalog,
        store_source_digest,
        engine_profile_digest,
        layout_epoch,
        store_commit_id: commit.map(|commit| commit.commit_id),
        changed_root_catalog_ids: discharge.changed_root_catalog_ids,
        changed_index_catalog_ids: discharge.changed_index_catalog_ids,
        verdicts: discharge.verdicts,
        counts: discharge.counts,
    };

    Ok((witness, discharge.diagnostics))
}

fn read_backup_witness_facts(path: &Path) -> Result<BackupWitnessFactSet, EvolutionPreviewError> {
    let mut file = std::fs::File::open(path)?;
    read_tree_backup_archive_header(&mut file)?;
    let _manifest =
        read_tree_backup_archive_chunk(&mut file, TREE_BACKUP_MAX_MANIFEST_BYTES, "manifest")?;
    let _catalog_section = read_tree_backup_archive_chunk(
        &mut file,
        TREE_BACKUP_MAX_CATALOG_SECTION_BYTES,
        "catalog section",
    )?;

    let mut cell_count = 0u64;
    let mut sample_catalog_ids = Vec::new();
    let mut samples_truncated = false;
    while let Some(cell) =
        TreeBackupCellBuf::read_framed_optional(&mut file, TREE_BACKUP_MAX_CELL_BYTES)?
    {
        cell_count += 1;
        samples_truncated |= sample_cell_catalog_ids(&cell, &mut sample_catalog_ids);
    }
    Ok(BackupWitnessFactSet {
        cell_count,
        sample_catalog_ids,
        samples_truncated,
    })
}

fn sample_cell_catalog_ids(cell: &TreeBackupCellBuf, samples: &mut Vec<String>) -> bool {
    let mut truncated = false;
    let key = cell.data_key();
    truncated |= push_sample(samples, key.store.as_str());
    match &key.kind {
        DataCellKind::Node => {}
        DataCellKind::PathNode { path } => {
            for segment in path {
                if let DataPathSegment::Member(member) = segment {
                    truncated |= push_sample(samples, member.as_str());
                }
            }
        }
        DataCellKind::Leaf { member } | DataCellKind::Sequence { member, .. } => {
            truncated |= push_sample(samples, member.as_str());
        }
        DataCellKind::Value { path } => {
            for segment in path {
                if let DataPathSegment::Member(member) = segment {
                    truncated |= push_sample(samples, member.as_str());
                }
            }
        }
    }
    truncated
}

fn push_sample(samples: &mut Vec<String>, id: &str) -> bool {
    if samples.iter().any(|sample| sample == id) {
        return false;
    }
    if samples.len() >= MAX_BACKUP_CATALOG_ID_SAMPLES {
        return true;
    }

    samples.push(id.to_string());
    false
}

#[cfg(test)]
mod tests {
    use marrow_store::tree::{CommitMetadata, TreeStore};

    use super::*;

    #[test]
    fn preview_keeps_an_empty_stamped_source_digest() {
        let store = TreeStore::memory();
        store
            .write_commit_metadata(&CommitMetadata {
                commit_id: 1,
                catalog_epoch: 7,
                layout_epoch: 0,
                source_digest: String::new(),
                engine_profile_digest: [0; 8],
                changed_root_catalog_ids: Vec::new(),
                changed_index_catalog_ids: Vec::new(),
            })
            .expect("write commit metadata");

        let (witness, diagnostics) = preview(&CheckedProgram::default(), &store).expect("preview");
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        assert_eq!(witness.store_source_digest, Some(String::new()));
        assert_eq!(witness.store_commit_id, Some(1));
    }
}
