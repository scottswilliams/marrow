//! Freezing a project's first accepted catalog into its store.
//!
//! The first authorized run over a genuinely empty store establishes the project's baseline
//! durable identity: it commits the program's first-run catalog proposal as the store's accepted
//! catalog and stamps the activation context in the same transaction the catalog rows land in.
//! When the project carries a committed `marrow.lock`, that proposal already holds the lock's
//! adopted identity and epoch high-water — adoption is resolved when the program is checked
//! against the lock, so a fresh checkout over a wiped store re-establishes the same identity
//! rather than minting a new one. The stamp is built through the same [`metadata_stamp`] the
//! evolution apply and managed-write paths use, so a store this path just wrote passes its own
//! open fence by construction. The committed store is the sole write-time authority; the caller
//! re-projects the lock from it after this commit.

use marrow_check::program::CheckedProgram;
use marrow_store::StoreError;
use marrow_store::tree::TreeStore;

use crate::write_plan::WritePlan;

use crate::write_plan::CommitIdAllocation;

use super::window::{StampFacts, metadata_stamp};

#[derive(Debug)]
pub enum BaselineError {
    Store(StoreError),
    Catalog(marrow_catalog::CatalogError),
}

impl BaselineError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Store(error) => error.code(),
            Self::Catalog(error) => error.code,
        }
    }
}

impl std::fmt::Display for BaselineError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => write!(formatter, "{error}"),
            Self::Catalog(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for BaselineError {}

impl From<StoreError> for BaselineError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<marrow_catalog::CatalogError> for BaselineError {
    fn from(error: marrow_catalog::CatalogError) -> Self {
        Self::Catalog(error)
    }
}

/// Commit the program's baseline accepted catalog, stamping the activation context in the
/// same transaction. The first-run proposal carries the project's baseline identity: when a
/// committed `marrow.lock` is present the proposal already holds its adopted ids and epoch
/// high-water (resolved at check time), so a fresh checkout over a wiped store re-establishes
/// the committed identity instead of minting a new one. Returns `Ok(true)` when the baseline was
/// written, and `Ok(false)` (writing nothing) when there is nothing to establish: the program has
/// no durable identity, the store already holds an accepted catalog (a project past its baseline
/// never churns the catalog rows or the commit stamp), or the store already holds saved data
/// without a catalog. That last case is a populated-but-unstamped store the caller must refuse
/// rather than silently adopt, so the baseline never stamps over it.
///
/// The catalog rows and commit metadata land under one transaction, so a reader sees
/// either no accepted catalog or the whole baseline. The commit metadata records no
/// activation work: a baseline freezes identity without backfilling, transforming, or
/// retiring any record.
pub fn commit_catalog_baseline(
    store: &TreeStore,
    program: &CheckedProgram,
) -> Result<bool, BaselineError> {
    let Some(snapshot) = baseline_snapshot(program)? else {
        return Ok(false);
    };
    if snapshot.entries.is_empty()
        || store.read_catalog_snapshot()?.is_some()
        || !store.is_empty()?
    {
        return Ok(false);
    }

    let stamp = metadata_stamp(StampFacts {
        catalog_epoch: snapshot.epoch,
        catalog_snapshot: Some(Box::new(snapshot)),
        commit_id: CommitIdAllocation::Baseline,
        source_digest: program.source_digest(),
        changed_root_catalog_ids: Vec::new(),
        changed_index_catalog_ids: Vec::new(),
    });

    WritePlan { steps: vec![stamp] }
        .commit(store, false)
        .map(|()| true)
        .map_err(BaselineError::from)
}

/// The catalog a baseline freezes into an empty store: the first-run proposal when the program
/// proposes one, otherwise the already-accepted snapshot the program checked against. The second
/// case re-establishes a known accepted identity into a freshly-emptied store (a test harness or
/// a backup-restore staging path) without re-deriving it.
fn baseline_snapshot(
    program: &CheckedProgram,
) -> Result<Option<marrow_catalog::CatalogMetadata>, marrow_catalog::CatalogError> {
    if let Some(proposal) = program.catalog.proposal.clone() {
        return Ok(Some(proposal));
    }
    let Some(epoch) = program.catalog.accepted_epoch else {
        return Ok(None);
    };
    marrow_catalog::CatalogMetadata::new(epoch, program.catalog.accepted_entries.clone()).map(Some)
}
