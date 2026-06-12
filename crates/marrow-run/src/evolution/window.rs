//! Activation-window fencing for write-capable store opens.
//!
//! A write-capable open fails closed unless the store's stamped catalog epoch, schema
//! source digest, and engine profile all match what this binary writes; a fresh
//! store has no stamp and is adopted on the first commit. The stamp this
//! module builds and the fence it enforces read the same facts, so a store this binary
//! just wrote always passes its own fence.

use marrow_catalog::CatalogMetadata;
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::tree::{ActivationDefaultRecordCount, CommitMetadata, EngineProfile, TreeStore};

use crate::write_plan::{ActivationEvidenceMode, CommitIdAllocation, PlanStep};

/// The canonical layout epoch the current binary writes. The apply stamp and the open
/// fence both derive their engine profile from this one constant, so a freshly applied
/// store always passes the fence under the same binary. It advances only when the
/// physical tree-cell layout changes in a way that makes older bytes unreadable.
const CANONICAL_LAYOUT_EPOCH: u64 = 0;

pub fn current_engine_profile() -> EngineProfile {
    EngineProfile::new(CANONICAL_LAYOUT_EPOCH)
}

/// The durable context a managed write or evolution apply records for the commit it
/// stamps. `catalog_snapshot` is the accepted catalog to publish atomically with the
/// stamp. `changed_root_catalog_ids` and `changed_index_catalog_ids` describe this
/// commit's own writes. Activation evidence is either explicit for an apply, carried
/// from the predecessor commit for an ordinary managed write, or empty.
pub(crate) struct StampFacts {
    pub(crate) catalog_epoch: u64,
    pub(crate) catalog_snapshot: Option<Box<CatalogMetadata>>,
    pub(crate) commit_id: CommitIdAllocation,
    pub(crate) source_digest: String,
    pub(crate) changed_root_catalog_ids: Vec<CatalogId>,
    pub(crate) changed_index_catalog_ids: Vec<CatalogId>,
    pub(crate) activation_evidence: StampActivationEvidence,
}

pub(crate) enum StampActivationEvidence {
    Empty,
    CarryPrevious,
    Applied(AppliedActivationEvidence),
}

#[derive(Default)]
pub(crate) struct AppliedActivationEvidence {
    pub(crate) evolution_digest: String,
    pub(crate) proposal_catalog_digest: Option<String>,
    pub(crate) proposal_new_catalog_ids: Vec<CatalogId>,
    pub(crate) records_backfilled: u64,
    pub(crate) default_records_by_id: Vec<ActivationDefaultRecordCount>,
    pub(crate) indexes_rebuilt: u64,
    pub(crate) records_retired: u64,
    pub(crate) retire_evidence_digest: String,
    pub(crate) records_retired_by_id: Vec<(CatalogId, u64)>,
    pub(crate) records_transformed: u64,
}

/// Build the metadata stamp both the runtime managed-write path and evolution apply
/// commit in the same transaction as their data. Pinning the engine profile and the
/// activated source digest here keeps the stamp and the open fence agreeing by
/// construction: the fence reads exactly the layout and digest this stamp wrote.
pub(crate) fn metadata_stamp(facts: StampFacts) -> PlanStep {
    let profile = current_engine_profile();
    let (activation_evidence, applied_activation_evidence) = match facts.activation_evidence {
        StampActivationEvidence::Empty => (
            ActivationEvidenceMode::Explicit,
            AppliedActivationEvidence::default(),
        ),
        StampActivationEvidence::CarryPrevious => (
            ActivationEvidenceMode::CarryPrevious,
            AppliedActivationEvidence::default(),
        ),
        StampActivationEvidence::Applied(evidence) => (ActivationEvidenceMode::Explicit, evidence),
    };
    let commit = CommitMetadata {
        commit_id: 0,
        catalog_epoch: facts.catalog_epoch,
        layout_epoch: profile.layout_epoch(),
        source_digest: facts.source_digest,
        engine_profile_digest: profile.digest_bytes(),
        changed_root_catalog_ids: facts.changed_root_catalog_ids,
        changed_index_catalog_ids: facts.changed_index_catalog_ids,
        activation_evolution_digest: applied_activation_evidence.evolution_digest,
        activation_proposal_catalog_digest: applied_activation_evidence.proposal_catalog_digest,
        activation_proposal_new_catalog_ids: applied_activation_evidence.proposal_new_catalog_ids,
        activation_records_backfilled: applied_activation_evidence.records_backfilled,
        activation_default_records_by_id: applied_activation_evidence.default_records_by_id,
        activation_indexes_rebuilt: applied_activation_evidence.indexes_rebuilt,
        activation_records_retired: applied_activation_evidence.records_retired,
        activation_retire_evidence_digest: applied_activation_evidence.retire_evidence_digest,
        activation_records_retired_by_id: applied_activation_evidence.records_retired_by_id,
        activation_records_transformed: applied_activation_evidence.records_transformed,
    };
    PlanStep::StampMetadata {
        catalog_snapshot: facts.catalog_snapshot,
        commit_id: facts.commit_id,
        activation_evidence,
        commit: Box::new(commit),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FenceError {
    StoreEvolved { stored: u64, accepted: u64 },
    StoreBehind { stored: u64, accepted: u64 },
    SchemaDrift,
    EngineProfileDrift,
    Store(StoreError),
}

impl From<StoreError> for FenceError {
    fn from(error: StoreError) -> Self {
        FenceError::Store(error)
    }
}

impl FenceError {
    pub fn code(&self) -> &'static str {
        match self {
            FenceError::StoreEvolved { .. } => "run.store_evolved",
            FenceError::StoreBehind { .. } => "run.store_behind",
            FenceError::SchemaDrift => "run.schema_drift",
            FenceError::EngineProfileDrift => "run.engine_profile",
            FenceError::Store(error) => error.code(),
        }
    }

    pub fn message(&self) -> String {
        match self {
            FenceError::StoreEvolved { stored, accepted } => format!(
                "store catalog epoch {stored} is newer than this program's accepted epoch {accepted}; \
                 the store was evolved by a newer binary. Recompile or upgrade against the current accepted catalog before running."
            ),
            FenceError::StoreBehind { stored, accepted } => format!(
                "store catalog epoch {stored} is older than this program's accepted epoch {accepted}; \
                 the store predates this catalog. Activate the store to epoch {accepted} with an evolution apply first."
            ),
            FenceError::SchemaDrift => {
                "store was stamped under a different schema at this catalog epoch; \
                 the durable shape this binary expects does not match the one the store holds"
                    .to_string()
            }
            FenceError::EngineProfileDrift => {
                "store engine profile does not match this binary's storage layout".to_string()
            }
            FenceError::Store(error) => error.to_string(),
        }
    }
}

/// Fence a write-capable store open against this binary's durable activation context.
///
/// The store's stamp pins three facts the binary must match: the engine profile (the
/// physical layout this binary writes), the catalog epoch (the accepted schema version),
/// and the source digest (the schema shape itself). The catalog epoch is a coarse
/// version number; two structurally different schemas can share an epoch, so the source
/// digest is the schema-bearing fence that distinguishes them. A fresh store has
/// no stamp and is adopted on the first commit.
pub fn fence(
    accepted_epoch: Option<u64>,
    expected_source_digest: &str,
    expected_profile: &EngineProfile,
    store: &TreeStore,
) -> Result<(), FenceError> {
    let Some(accepted) = accepted_epoch else {
        return Ok(());
    };

    let Some(commit) = store.read_commit_metadata()? else {
        return Ok(());
    };
    if commit.layout_epoch != expected_profile.layout_epoch()
        || commit.engine_profile_digest != expected_profile.digest_bytes()
    {
        return Err(FenceError::EngineProfileDrift);
    }

    let stored = commit.catalog_epoch;
    match stored.cmp(&accepted) {
        std::cmp::Ordering::Greater => return Err(FenceError::StoreEvolved { stored, accepted }),
        std::cmp::Ordering::Less => return Err(FenceError::StoreBehind { stored, accepted }),
        std::cmp::Ordering::Equal => {}
    }

    if commit.source_digest != expected_source_digest {
        return Err(FenceError::SchemaDrift);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{FenceError, current_engine_profile, fence};
    use marrow_store::tree::{CommitMetadata, EngineProfile, TreeStore};

    const DIGEST: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000001";

    fn stamp_with_digest(store: &TreeStore, epoch: u64, digest: &str) {
        store
            .write_commit_metadata(&commit_metadata(epoch, digest))
            .expect("write commit metadata");
    }

    fn commit_metadata(epoch: u64, digest: &str) -> CommitMetadata {
        let profile = current_engine_profile();
        CommitMetadata {
            commit_id: 0,
            catalog_epoch: epoch,
            layout_epoch: profile.layout_epoch(),
            source_digest: digest.to_string(),
            engine_profile_digest: profile.digest_bytes(),
            changed_root_catalog_ids: Vec::new(),
            changed_index_catalog_ids: Vec::new(),
            activation_evolution_digest: String::new(),
            activation_proposal_catalog_digest: None,
            activation_proposal_new_catalog_ids: Vec::new(),
            activation_records_backfilled: 0,
            activation_default_records_by_id: Vec::new(),
            activation_indexes_rebuilt: 0,
            activation_records_retired: 0,
            activation_retire_evidence_digest: String::new(),
            activation_records_retired_by_id: Vec::new(),
            activation_records_transformed: 0,
        }
    }

    #[test]
    fn store_evolved_past_program_is_fenced() {
        let store = TreeStore::memory();
        stamp_with_digest(&store, 5, DIGEST);
        let error = fence(Some(3), DIGEST, &current_engine_profile(), &store).expect_err("fenced");
        assert_eq!(
            error,
            FenceError::StoreEvolved {
                stored: 5,
                accepted: 3
            }
        );
        assert_eq!(error.code(), "run.store_evolved");
    }

    #[test]
    fn store_behind_program_is_fenced() {
        let store = TreeStore::memory();
        stamp_with_digest(&store, 2, DIGEST);
        let error = fence(Some(4), DIGEST, &current_engine_profile(), &store).expect_err("fenced");
        assert_eq!(
            error,
            FenceError::StoreBehind {
                stored: 2,
                accepted: 4
            }
        );
        assert_eq!(error.code(), "run.store_behind");
    }

    #[test]
    fn store_at_program_epoch_proceeds() {
        let store = TreeStore::memory();
        stamp_with_digest(&store, 7, DIGEST);
        fence(Some(7), DIGEST, &current_engine_profile(), &store).expect("proceeds");
    }

    #[test]
    fn schema_drift_at_the_same_epoch_is_fenced() {
        let store = TreeStore::memory();
        stamp_with_digest(&store, 7, DIGEST);
        let error = fence(
            Some(7),
            "sha256:00000000000000000000000000000000000000000000000000000000deadbeef",
            &current_engine_profile(),
            &store,
        )
        .expect_err("fenced");
        assert_eq!(error, FenceError::SchemaDrift);
        assert_eq!(error.code(), "run.schema_drift");
    }

    #[test]
    fn commit_metadata_alone_marks_store_as_stamped() {
        let store = TreeStore::memory();
        store
            .write_commit_metadata(&commit_metadata(7, DIGEST))
            .expect("write commit metadata");

        let error = fence(
            Some(7),
            "sha256:00000000000000000000000000000000000000000000000000000000deadbeef",
            &current_engine_profile(),
            &store,
        )
        .expect_err("commit metadata is the stamp");

        assert_eq!(error, FenceError::SchemaDrift);
        assert_eq!(error.code(), "run.schema_drift");
    }

    #[test]
    fn epoch_match_without_commit_metadata_is_adopted() {
        let store = TreeStore::memory();
        fence(Some(7), DIGEST, &current_engine_profile(), &store)
            .expect("a store with no commit metadata is adopted");
    }

    #[test]
    fn epoch_stamped_receipt_with_empty_digest_is_schema_drift() {
        let store = TreeStore::memory();
        stamp_with_digest(&store, 7, "");
        let error = fence(Some(7), DIGEST, &current_engine_profile(), &store)
            .expect_err("an empty stamped digest is a schema drift receipt");
        assert_eq!(error, FenceError::SchemaDrift);
        assert_eq!(error.code(), "run.schema_drift");
    }

    #[test]
    fn engine_profile_mismatch_is_fenced() {
        let store = TreeStore::memory();
        let mut commit = commit_metadata(3, DIGEST);
        let drifted = EngineProfile::new(current_engine_profile().layout_epoch() + 1);
        commit.layout_epoch = drifted.layout_epoch();
        commit.engine_profile_digest = drifted.digest_bytes();
        store
            .write_commit_metadata(&commit)
            .expect("write drifted commit metadata");
        let error = fence(Some(3), DIGEST, &current_engine_profile(), &store).expect_err("fenced");
        assert_eq!(error, FenceError::EngineProfileDrift);
        assert_eq!(error.code(), "run.engine_profile");
    }

    #[test]
    fn unstamped_store_is_adopted() {
        let store = TreeStore::memory();
        fence(Some(9), DIGEST, &current_engine_profile(), &store).expect("adopts a fresh store");
    }

    #[test]
    fn no_accepted_catalog_does_not_fence() {
        let store = TreeStore::memory();
        stamp_with_digest(
            &store,
            5,
            "sha256:00000000000000000000000000000000000000000000000000000000deadbeef",
        );
        fence(None, DIGEST, &current_engine_profile(), &store).expect("no catalog, no fence");
    }
}
