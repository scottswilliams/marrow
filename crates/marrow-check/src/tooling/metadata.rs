use marrow_store::StoreError;
use marrow_store::tree::{EngineProfileDigest, TreeStore};

use crate::CheckedProgram;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolingCatalogMetadata {
    pub source_digest: String,
    pub accepted_catalog_epoch: Option<u64>,
    pub store_catalog_epoch: Option<u64>,
    pub layout_epoch: Option<u64>,
    pub engine_profile_digest: Option<EngineProfileDigest>,
}

pub fn tooling_metadata(
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<ToolingCatalogMetadata, StoreError> {
    Ok(ToolingCatalogMetadata {
        source_digest: program.source_digest(),
        accepted_catalog_epoch: program.catalog.accepted_epoch,
        store_catalog_epoch: store.read_catalog_epoch()?,
        layout_epoch: store.read_layout_epoch()?,
        engine_profile_digest: store.read_engine_profile_digest()?,
    })
}

pub fn store_is_newer_than_program(metadata: &ToolingCatalogMetadata) -> bool {
    let Some(stored) = metadata.store_catalog_epoch else {
        return false;
    };
    match metadata.accepted_catalog_epoch {
        Some(accepted) => stored > accepted,
        None => true,
    }
}
