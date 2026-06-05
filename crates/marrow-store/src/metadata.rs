use crate::StoreError;
use crate::cell::CatalogId;

const ENGINE_PROFILE_KEY_VERSION_V0: u8 = 0;
const ENGINE_PROFILE_DIGEST_BYTES: usize = 8;
const MIN_ENCODED_CATALOG_ID_BYTES: usize = 4 + "cat_00000000000000000000000000000000".len();
const MIN_LENGTH_PREFIX_BYTES: usize = 4;

pub type EngineProfileDigest = [u8; ENGINE_PROFILE_DIGEST_BYTES];

/// The engine profile recorded with tree-cell metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineProfile {
    layout_epoch: u64,
}

impl EngineProfile {
    pub fn new(layout_epoch: u64) -> Self {
        Self { layout_epoch }
    }

    pub fn layout_epoch(&self) -> u64 {
        self.layout_epoch
    }

    pub fn key_profile_version(&self) -> u8 {
        ENGINE_PROFILE_KEY_VERSION_V0
    }

    pub fn digest_bytes(&self) -> EngineProfileDigest {
        engine_profile_hash64(&self.digest_preimage()).to_be_bytes()
    }

    pub fn digest_hex(&self) -> String {
        let digest = u64::from_be_bytes(self.digest_bytes());
        format!("{digest:016x}")
    }

    fn digest_preimage(&self) -> Vec<u8> {
        let mut bytes = b"marrow-tree-cell-engine-profile-v0".to_vec();
        bytes.push(0);
        bytes.push(self.key_profile_version());
        bytes.extend_from_slice(&self.layout_epoch.to_be_bytes());
        bytes
    }
}

/// Metadata recorded for the latest tree-cell commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitMetadata {
    pub commit_id: u64,
    pub catalog_epoch: u64,
    pub layout_epoch: u64,
    /// The analyzed-source digest the commit activated, in the `sha256:<hex>` form the
    /// evolution witness records. It binds the schema shape (member types, identity key
    /// types, index uniqueness and columns) the store was last written against, so the
    /// activation fence can reject a structurally different schema even at the same
    /// catalog epoch.
    pub source_digest: String,
    pub engine_profile_digest: EngineProfileDigest,
    pub changed_root_catalog_ids: Vec<CatalogId>,
    pub changed_index_catalog_ids: Vec<CatalogId>,
    pub activation_evolution_digest: String,
    pub activation_proposal_catalog_digest: Option<String>,
    pub activation_proposal_new_catalog_ids: Vec<CatalogId>,
    pub activation_records_backfilled: u64,
    pub activation_default_records_by_id: Vec<ActivationDefaultRecordCount>,
    pub activation_indexes_rebuilt: u64,
    pub activation_records_retired: u64,
    pub activation_retire_evidence_digest: String,
    pub activation_records_retired_by_id: Vec<(CatalogId, u64)>,
    pub activation_records_transformed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationDefaultRecordCount {
    pub catalog_id: CatalogId,
    pub records_backfilled: u64,
    pub target_records: u64,
    pub evidence_digest: String,
}

pub(crate) fn encode_commit_metadata(metadata: &CommitMetadata) -> Result<Vec<u8>, StoreError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&metadata.commit_id.to_be_bytes());
    bytes.extend_from_slice(&metadata.catalog_epoch.to_be_bytes());
    bytes.extend_from_slice(&metadata.layout_epoch.to_be_bytes());
    put_bytes(metadata.source_digest.as_bytes(), &mut bytes)?;
    put_bytes(&metadata.engine_profile_digest, &mut bytes)?;
    put_catalog_ids(&metadata.changed_root_catalog_ids, &mut bytes)?;
    put_catalog_ids(&metadata.changed_index_catalog_ids, &mut bytes)?;
    put_bytes(metadata.activation_evolution_digest.as_bytes(), &mut bytes)?;
    put_bytes(
        metadata
            .activation_proposal_catalog_digest
            .as_deref()
            .unwrap_or("")
            .as_bytes(),
        &mut bytes,
    )?;
    bytes.extend_from_slice(&metadata.activation_records_backfilled.to_be_bytes());
    bytes.extend_from_slice(&metadata.activation_indexes_rebuilt.to_be_bytes());
    bytes.extend_from_slice(&metadata.activation_records_retired.to_be_bytes());
    bytes.extend_from_slice(&metadata.activation_records_transformed.to_be_bytes());
    put_bytes(
        metadata.activation_retire_evidence_digest.as_bytes(),
        &mut bytes,
    )?;
    put_retire_counts(&metadata.activation_records_retired_by_id, &mut bytes)?;
    put_default_counts(&metadata.activation_default_records_by_id, &mut bytes)?;
    put_catalog_ids(&metadata.activation_proposal_new_catalog_ids, &mut bytes)?;
    Ok(bytes)
}

pub(crate) fn decode_commit_metadata(bytes: &[u8]) -> Result<CommitMetadata, StoreError> {
    let mut cursor = MetadataCursor::new(bytes);
    let commit_id = cursor.take_u64()?;
    let catalog_epoch = cursor.take_u64()?;
    let layout_epoch = cursor.take_u64()?;
    let source_digest = cursor.take_string()?;
    let engine_profile_digest = cursor.take_digest()?;
    let changed_root_catalog_ids = cursor.take_catalog_ids()?;
    let changed_index_catalog_ids = cursor.take_catalog_ids()?;
    let activation_evolution_digest = cursor.take_string()?;
    let proposal_digest = cursor.take_string()?;
    let activation_records_backfilled = cursor.take_u64()?;
    let activation_indexes_rebuilt = cursor.take_u64()?;
    let activation_records_retired = cursor.take_u64()?;
    let activation_records_transformed = cursor.take_u64()?;
    let activation_retire_evidence_digest = cursor.take_string()?;
    let activation_records_retired_by_id = cursor.take_retire_counts()?;
    let activation_default_records_by_id = cursor.take_default_counts()?;
    let activation_proposal_new_catalog_ids = cursor.take_catalog_ids()?;
    let activation_proposal_catalog_digest =
        (!proposal_digest.is_empty()).then_some(proposal_digest);
    if !cursor.is_empty() {
        return Err(corrupt_metadata(bytes));
    }
    Ok(CommitMetadata {
        commit_id,
        catalog_epoch,
        layout_epoch,
        source_digest,
        engine_profile_digest,
        changed_root_catalog_ids,
        changed_index_catalog_ids,
        activation_evolution_digest,
        activation_proposal_catalog_digest,
        activation_proposal_new_catalog_ids,
        activation_records_backfilled,
        activation_default_records_by_id,
        activation_indexes_rebuilt,
        activation_records_retired,
        activation_retire_evidence_digest,
        activation_records_retired_by_id,
        activation_records_transformed,
    })
}

fn engine_profile_hash64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn put_bytes(value: &[u8], out: &mut Vec<u8>) -> Result<(), StoreError> {
    let len = u32::try_from(value.len()).map_err(|_| StoreError::LimitExceeded {
        limit: "tree cell metadata length",
    })?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

fn put_catalog_ids(ids: &[CatalogId], out: &mut Vec<u8>) -> Result<(), StoreError> {
    let len = u32::try_from(ids.len()).map_err(|_| StoreError::LimitExceeded {
        limit: "tree cell metadata length",
    })?;
    out.extend_from_slice(&len.to_be_bytes());
    for id in ids {
        put_bytes(id.as_str().as_bytes(), out)?;
    }
    Ok(())
}

fn put_retire_counts(counts: &[(CatalogId, u64)], out: &mut Vec<u8>) -> Result<(), StoreError> {
    let len = u32::try_from(counts.len()).map_err(|_| StoreError::LimitExceeded {
        limit: "tree cell metadata length",
    })?;
    out.extend_from_slice(&len.to_be_bytes());
    for (id, count) in counts {
        put_bytes(id.as_str().as_bytes(), out)?;
        out.extend_from_slice(&count.to_be_bytes());
    }
    Ok(())
}

fn put_default_counts(
    counts: &[ActivationDefaultRecordCount],
    out: &mut Vec<u8>,
) -> Result<(), StoreError> {
    let len = u32::try_from(counts.len()).map_err(|_| StoreError::LimitExceeded {
        limit: "tree cell metadata length",
    })?;
    out.extend_from_slice(&len.to_be_bytes());
    for count in counts {
        put_bytes(count.catalog_id.as_str().as_bytes(), out)?;
        out.extend_from_slice(&count.records_backfilled.to_be_bytes());
        out.extend_from_slice(&count.target_records.to_be_bytes());
        put_bytes(count.evidence_digest.as_bytes(), out)?;
    }
    Ok(())
}

fn decode_digest(bytes: &[u8]) -> Result<EngineProfileDigest, StoreError> {
    bytes.try_into().map_err(|_| corrupt_metadata(bytes))
}

struct MetadataCursor<'a> {
    bytes: &'a [u8],
}

impl<'a> MetadataCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    fn take_u64(&mut self) -> Result<u64, StoreError> {
        let bytes = self.take(8)?;
        let raw: [u8; 8] = bytes.try_into().map_err(|_| corrupt_metadata(bytes))?;
        Ok(u64::from_be_bytes(raw))
    }

    fn take_bytes(&mut self) -> Result<&'a [u8], StoreError> {
        let len = self.take_u32()? as usize;
        self.take(len)
    }

    fn take_digest(&mut self) -> Result<EngineProfileDigest, StoreError> {
        decode_digest(self.take_bytes()?)
    }

    fn take_string(&mut self) -> Result<String, StoreError> {
        let raw = self.take_bytes()?;
        std::str::from_utf8(raw)
            .map(str::to_string)
            .map_err(|_| corrupt_metadata(raw))
    }

    fn take_catalog_id(&mut self) -> Result<CatalogId, StoreError> {
        let raw = self.take_bytes()?;
        let id = std::str::from_utf8(raw).map_err(|_| corrupt_metadata(raw))?;
        CatalogId::new(id).map_err(|_| corrupt_metadata(raw))
    }

    fn take_catalog_ids(&mut self) -> Result<Vec<CatalogId>, StoreError> {
        let len = self.take_u32()? as usize;
        if len > self.bytes.len() / MIN_ENCODED_CATALOG_ID_BYTES {
            return Err(corrupt_metadata(self.bytes));
        }
        let mut ids = Vec::new();
        for _ in 0..len {
            let raw = self.take_bytes()?;
            let id = std::str::from_utf8(raw).map_err(|_| corrupt_metadata(raw))?;
            ids.push(CatalogId::new(id).map_err(|_| corrupt_metadata(raw))?);
        }
        Ok(ids)
    }

    fn take_retire_counts(&mut self) -> Result<Vec<(CatalogId, u64)>, StoreError> {
        let len = self.take_u32()? as usize;
        if len > self.bytes.len() / (MIN_ENCODED_CATALOG_ID_BYTES + 8) {
            return Err(corrupt_metadata(self.bytes));
        }
        let mut counts = Vec::new();
        for _ in 0..len {
            counts.push((self.take_catalog_id()?, self.take_u64()?));
        }
        Ok(counts)
    }

    fn take_default_counts(&mut self) -> Result<Vec<ActivationDefaultRecordCount>, StoreError> {
        let len = self.take_u32()? as usize;
        if len > self.bytes.len() / (MIN_ENCODED_CATALOG_ID_BYTES + 16 + MIN_LENGTH_PREFIX_BYTES) {
            return Err(corrupt_metadata(self.bytes));
        }
        let mut counts = Vec::new();
        for _ in 0..len {
            counts.push(ActivationDefaultRecordCount {
                catalog_id: self.take_catalog_id()?,
                records_backfilled: self.take_u64()?,
                target_records: self.take_u64()?,
                evidence_digest: self.take_string()?,
            });
        }
        Ok(counts)
    }

    fn take_u32(&mut self) -> Result<u32, StoreError> {
        let bytes = self.take(4)?;
        let raw: [u8; 4] = bytes.try_into().map_err(|_| corrupt_metadata(bytes))?;
        Ok(u32::from_be_bytes(raw))
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], StoreError> {
        let Some((head, tail)) = self.bytes.split_at_checked(len) else {
            return Err(corrupt_metadata(self.bytes));
        };
        self.bytes = tail;
        Ok(head)
    }
}

fn corrupt_metadata(bytes: &[u8]) -> StoreError {
    StoreError::Corruption {
        message: format!("tree-cell metadata is malformed ({} bytes)", bytes.len()),
    }
}
