use crate::StoreError;
use crate::cell::CatalogId;
use crate::codec::BoundedReader;
use crate::digest::RootDigest;

const ENGINE_PROFILE_KEY_VERSION_V0: u8 = 0;
const ENGINE_PROFILE_DIGEST_BYTES: usize = 8;
const MIN_ENCODED_CATALOG_ID_BYTES: usize = 4 + "cat_00000000000000000000000000000000".len();
const STORE_UID_PREFIX: &str = "store_";
const STORE_UID_HEX_LEN: usize = 32;
const LOWER_HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

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
        let bytes = self.digest_bytes();
        let mut digest = String::with_capacity(ENGINE_PROFILE_DIGEST_BYTES * 2);
        push_lower_hex(&mut digest, &bytes);
        digest
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
}

/// The sealed commit record: the single durable witness the store-open path validates.
///
/// It binds the store's identity, the accepted-catalog epoch and digest it was last committed
/// at, its active saved roots, and each root's structural digest under one content seal. Because
/// a localized backend flip of any bound field breaks the seal, a store opens by validating this
/// one record — recompute the seal, then cross-check its bound fields against the store's own uid,
/// commit stamp, and catalog snapshot — instead of scanning every cell. The seal is a content hash,
/// not a MAC: it detects backend damage, not a hostile re-forge, matching the corruption threat
/// model every other durable digest here defends against. The per-root digests live here rather
/// than in a separate anchor family, so the record is their single durable home.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct CommitRecord {
    pub(crate) store_uid: Option<StoreUid>,
    pub(crate) catalog_epoch: Option<u64>,
    pub(crate) catalog_digest: Option<String>,
    pub(crate) active_roots: Vec<CatalogId>,
    pub(crate) root_digests: Vec<(CatalogId, RootDigest)>,
}

const COMMIT_RECORD_VERSION_V0: u8 = 0;
const COMMIT_RECORD_SEAL_BYTES: usize = 16;
const MIN_ENCODED_ROOT_DIGEST_BYTES: usize = MIN_ENCODED_CATALOG_ID_BYTES + RootDigest::ENCODED_LEN;

/// Stable identity for one physical store, spelled `store_<32 lowercase hex>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoreUid(String);

impl StoreUid {
    pub fn new(uid: impl Into<String>) -> Result<Self, StoreUidError> {
        let uid = uid.into();
        let Some(hex) = uid.strip_prefix(STORE_UID_PREFIX) else {
            return Err(StoreUidError);
        };
        if hex.len() != STORE_UID_HEX_LEN
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(StoreUidError);
        }
        Ok(Self(uid))
    }

    pub fn from_entropy_bytes(bytes: [u8; 16]) -> Self {
        let mut uid = String::with_capacity(STORE_UID_PREFIX.len() + STORE_UID_HEX_LEN);
        uid.push_str(STORE_UID_PREFIX);
        push_lower_hex(&mut uid, &bytes);
        Self(uid)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreUidError;

impl std::fmt::Display for StoreUidError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("store UIDs must match store_<32 lowercase hex>")
    }
}

impl std::error::Error for StoreUidError {}

fn push_lower_hex(out: &mut String, bytes: &[u8]) {
    for &byte in bytes {
        out.push(char::from(LOWER_HEX_DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(LOWER_HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
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
    Ok(bytes)
}

pub(crate) fn decode_commit_metadata(bytes: &[u8]) -> Result<CommitMetadata, StoreError> {
    let mut cursor = BoundedReader::new(bytes, corrupt_metadata);
    let commit_id = cursor.take_u64()?;
    let catalog_epoch = cursor.take_u64()?;
    let layout_epoch = cursor.take_u64()?;
    let source_digest = take_string(&mut cursor)?;
    let engine_profile_digest = take_digest(&mut cursor)?;
    let changed_root_catalog_ids = take_catalog_ids(&mut cursor)?;
    let changed_index_catalog_ids = take_catalog_ids(&mut cursor)?;
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
    })
}

/// Encode the sealed commit record as `[seal][body]`, where the seal is a content hash over the
/// exact body bytes. Decode recomputes the seal over the same body, so any flip in a bound field
/// or in the seal itself fails closed.
pub(crate) fn encode_commit_record(record: &CommitRecord) -> Result<Vec<u8>, StoreError> {
    let mut body = vec![COMMIT_RECORD_VERSION_V0];
    put_optional_bytes(record.store_uid.as_ref().map(StoreUid::as_str), &mut body)?;
    put_optional_u64(record.catalog_epoch, &mut body);
    put_optional_bytes(record.catalog_digest.as_deref(), &mut body)?;
    put_catalog_ids(&record.active_roots, &mut body)?;
    put_root_digests(&record.root_digests, &mut body)?;
    let seal = seal_bytes(&body);
    let mut bytes = Vec::with_capacity(seal.len() + body.len());
    bytes.extend_from_slice(&seal);
    bytes.extend_from_slice(&body);
    Ok(bytes)
}

pub(crate) fn decode_commit_record(bytes: &[u8]) -> Result<CommitRecord, StoreError> {
    let mut cursor = BoundedReader::new(bytes, corrupt_record);
    let seal = cursor.take_array::<COMMIT_RECORD_SEAL_BYTES>()?;
    let body = cursor.take_rest();
    if seal_bytes(body) != seal {
        return Err(corrupt_record(bytes));
    }
    let mut cursor = BoundedReader::new(body, corrupt_record);
    if cursor.take_u8()? != COMMIT_RECORD_VERSION_V0 {
        return Err(corrupt_record(body));
    }
    let store_uid = take_optional(&mut cursor, decode_store_uid)?;
    let catalog_epoch = take_optional_u64(&mut cursor)?;
    let catalog_digest = take_optional(&mut cursor, |raw| {
        std::str::from_utf8(raw)
            .map(str::to_string)
            .map_err(|_| corrupt_record(raw))
    })?;
    let active_roots = take_catalog_ids(&mut cursor)?;
    let root_digests = take_root_digests(&mut cursor)?;
    if !cursor.is_empty() {
        return Err(corrupt_record(body));
    }
    Ok(CommitRecord {
        store_uid,
        catalog_epoch,
        catalog_digest,
        active_roots,
        root_digests,
    })
}

/// A 128-bit content seal over the record body: two FNV-1a streams over distinct bases, so a
/// single flipped body byte diverges the result. The two streams share the recurrence and differ
/// only in the basis, so the halves are correlated rather than an independent 128 bits — immaterial
/// for the store's threat model, which is accidental backend corruption, not a keyed adversary, so
/// a content hash — not a MAC — is the right primitive, as for every other durable digest here.
fn seal_bytes(body: &[u8]) -> [u8; COMMIT_RECORD_SEAL_BYTES] {
    const BASIS_HI: u64 = 0xcbf2_9ce4_8422_2325;
    const BASIS_LO: u64 = 0x9e37_79b9_7f4a_7c15;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hi = BASIS_HI;
    let mut lo = BASIS_LO;
    for &byte in body {
        hi = (hi ^ u64::from(byte)).wrapping_mul(PRIME);
        lo = (lo ^ u64::from(byte)).wrapping_mul(PRIME);
    }
    let mut seal = [0u8; COMMIT_RECORD_SEAL_BYTES];
    seal[..8].copy_from_slice(&hi.to_be_bytes());
    seal[8..].copy_from_slice(&lo.to_be_bytes());
    seal
}

fn put_optional_bytes(value: Option<&str>, out: &mut Vec<u8>) -> Result<(), StoreError> {
    match value {
        Some(value) => {
            out.push(1);
            put_bytes(value.as_bytes(), out)
        }
        None => {
            out.push(0);
            Ok(())
        }
    }
}

fn put_optional_u64(value: Option<u64>, out: &mut Vec<u8>) {
    match value {
        Some(value) => {
            out.push(1);
            out.extend_from_slice(&value.to_be_bytes());
        }
        None => out.push(0),
    }
}

fn put_root_digests(
    digests: &[(CatalogId, RootDigest)],
    out: &mut Vec<u8>,
) -> Result<(), StoreError> {
    let len = u32::try_from(digests.len()).map_err(|_| StoreError::LimitExceeded {
        limit: "tree cell metadata length",
    })?;
    out.extend_from_slice(&len.to_be_bytes());
    for (store, digest) in digests {
        put_bytes(store.as_str().as_bytes(), out)?;
        out.extend_from_slice(&digest.to_be_bytes());
    }
    Ok(())
}

fn take_optional<T>(
    cursor: &mut MetadataReader<'_>,
    decode: impl Fn(&[u8]) -> Result<T, StoreError>,
) -> Result<Option<T>, StoreError> {
    match cursor.take_u8()? {
        0 => Ok(None),
        1 => Ok(Some(decode(cursor.take_prefixed_bytes()?)?)),
        _ => Err(corrupt_record(&[])),
    }
}

fn take_optional_u64(cursor: &mut MetadataReader<'_>) -> Result<Option<u64>, StoreError> {
    match cursor.take_u8()? {
        0 => Ok(None),
        1 => Ok(Some(cursor.take_u64()?)),
        _ => Err(corrupt_record(&[])),
    }
}

fn take_root_digests(
    cursor: &mut MetadataReader<'_>,
) -> Result<Vec<(CatalogId, RootDigest)>, StoreError> {
    let len = cursor.take_bounded_count(MIN_ENCODED_ROOT_DIGEST_BYTES)?;
    (0..len)
        .map(|_| {
            let store = take_catalog_id(cursor)?;
            let digest = RootDigest::from_be_bytes(cursor.take_array()?);
            Ok((store, digest))
        })
        .collect()
}

fn corrupt_record(bytes: &[u8]) -> StoreError {
    StoreError::Corruption {
        message: format!("commit record is malformed ({} bytes)", bytes.len()),
    }
}

pub(crate) fn encode_store_uid(uid: &StoreUid) -> Vec<u8> {
    uid.as_str().as_bytes().to_vec()
}

pub(crate) fn decode_store_uid(bytes: &[u8]) -> Result<StoreUid, StoreError> {
    let uid = std::str::from_utf8(bytes).map_err(|_| corrupt_metadata(bytes))?;
    StoreUid::new(uid.to_string()).map_err(|_| corrupt_metadata(bytes))
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

fn decode_digest(bytes: &[u8]) -> Result<EngineProfileDigest, StoreError> {
    bytes.try_into().map_err(|_| corrupt_metadata(bytes))
}

type MetadataReader<'a> = BoundedReader<'a, StoreError>;

fn take_digest(cursor: &mut MetadataReader<'_>) -> Result<EngineProfileDigest, StoreError> {
    decode_digest(cursor.take_prefixed_bytes()?)
}

fn take_string(cursor: &mut MetadataReader<'_>) -> Result<String, StoreError> {
    let raw = cursor.take_prefixed_bytes()?;
    std::str::from_utf8(raw)
        .map(str::to_string)
        .map_err(|_| corrupt_metadata(raw))
}

fn take_catalog_id(cursor: &mut MetadataReader<'_>) -> Result<CatalogId, StoreError> {
    let raw = cursor.take_prefixed_bytes()?;
    let id = std::str::from_utf8(raw).map_err(|_| corrupt_metadata(raw))?;
    CatalogId::new(id).map_err(|_| corrupt_metadata(raw))
}

fn take_catalog_ids(cursor: &mut MetadataReader<'_>) -> Result<Vec<CatalogId>, StoreError> {
    let len = cursor.take_bounded_count(MIN_ENCODED_CATALOG_ID_BYTES)?;
    (0..len).map(|_| take_catalog_id(cursor)).collect()
}

fn corrupt_metadata(bytes: &[u8]) -> StoreError {
    StoreError::Corruption {
        message: format!("tree-cell metadata is malformed ({} bytes)", bytes.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CommitMetadata, EngineProfile, StoreUid, decode_commit_metadata, encode_commit_metadata,
    };
    use crate::StoreError;
    use crate::cell::CatalogId;

    fn catalog_id(suffix: &str) -> CatalogId {
        CatalogId::new(format!("cat_{suffix:0>32}")).expect("catalog id")
    }

    fn rich_commit_metadata() -> CommitMetadata {
        let profile = EngineProfile::new(5);
        CommitMetadata {
            commit_id: 9,
            catalog_epoch: 4,
            layout_epoch: 5,
            source_digest: "sha256:source".into(),
            engine_profile_digest: profile.digest_bytes(),
            changed_root_catalog_ids: vec![catalog_id("1"), catalog_id("2")],
            changed_index_catalog_ids: vec![catalog_id("3")],
        }
    }

    #[test]
    fn engine_profile_digest_hex_uses_canonical_text() {
        assert_eq!(EngineProfile::new(5).digest_hex(), "779449b86c08ade6");
    }

    #[test]
    fn store_uid_from_entropy_bytes_uses_canonical_text() {
        let uid = StoreUid::from_entropy_bytes([
            0x00, 0x01, 0x02, 0x09, 0x0a, 0x0f, 0x10, 0x11, 0x80, 0x90, 0xa0, 0xb0, 0xc0, 0xd0,
            0xe0, 0xff,
        ]);

        assert_eq!(uid.as_str(), "store_000102090a0f10118090a0b0c0d0e0ff");
    }

    #[test]
    fn store_uid_parser_rejects_non_canonical_text() {
        for uid in [
            "cat_000102090a0f10118090a0b0c0d0e0ff",
            "store_000102090a0f10118090a0b0c0d0e0f",
            "store_000102090a0f10118090a0b0c0d0e0ff0",
            "store_000102090a0f10118090a0b0c0d0e0fG",
            "store_000102090A0f10118090a0b0c0d0e0ff",
        ] {
            assert!(StoreUid::new(uid).is_err(), "{uid}");
        }
    }

    #[test]
    fn commit_metadata_codec_round_trips_every_field() {
        let metadata = rich_commit_metadata();
        let bytes = encode_commit_metadata(&metadata).expect("metadata encodes");

        assert_eq!(
            decode_commit_metadata(&bytes).expect("metadata decodes"),
            metadata
        );
    }

    #[test]
    fn commit_metadata_codec_rejects_trailing_bytes() {
        let metadata = rich_commit_metadata();
        let mut bytes = encode_commit_metadata(&metadata).expect("metadata encodes");
        bytes.push(0);

        assert!(matches!(
            decode_commit_metadata(&bytes),
            Err(StoreError::Corruption { .. })
        ));
    }

    fn rich_commit_record() -> super::CommitRecord {
        use crate::digest::RootDigest;
        let mut first = RootDigest::zero();
        first.add_cell(b"k1", b"v1");
        let mut second = RootDigest::zero();
        second.add_cell(b"k2", b"v2");
        super::CommitRecord {
            store_uid: Some(StoreUid::from_entropy_bytes([7; 16])),
            catalog_epoch: Some(4),
            catalog_digest: Some("sha256:cafe".into()),
            active_roots: vec![catalog_id("1"), catalog_id("2")],
            root_digests: vec![(catalog_id("1"), first), (catalog_id("2"), second)],
        }
    }

    #[test]
    fn commit_record_codec_round_trips_every_field() {
        use super::{decode_commit_record, encode_commit_record};
        let record = rich_commit_record();
        let bytes = encode_commit_record(&record).expect("record encodes");
        assert_eq!(
            decode_commit_record(&bytes).expect("record decodes"),
            record
        );
    }

    #[test]
    fn commit_record_codec_round_trips_an_empty_record() {
        use super::{CommitRecord, decode_commit_record, encode_commit_record};
        let record = CommitRecord::default();
        let bytes = encode_commit_record(&record).expect("record encodes");
        assert_eq!(
            decode_commit_record(&bytes).expect("record decodes"),
            record
        );
    }

    #[test]
    fn a_flip_of_any_sealed_byte_fails_closed() {
        use super::{decode_commit_record, encode_commit_record};
        let bytes = encode_commit_record(&rich_commit_record()).expect("record encodes");
        for index in 0..bytes.len() {
            let mut corrupt = bytes.clone();
            corrupt[index] ^= 0x01;
            assert!(
                matches!(
                    decode_commit_record(&corrupt),
                    Err(StoreError::Corruption { .. })
                ),
                "a flip at byte {index} must fail the seal"
            );
        }
    }

    #[test]
    fn commit_record_codec_rejects_trailing_bytes() {
        use super::{decode_commit_record, encode_commit_record};
        let mut bytes = encode_commit_record(&rich_commit_record()).expect("record encodes");
        bytes.push(0);
        assert!(matches!(
            decode_commit_record(&bytes),
            Err(StoreError::Corruption { .. })
        ));
    }

    #[test]
    fn commit_metadata_codec_carries_only_the_activation_stamp() {
        let metadata = rich_commit_metadata();
        let bytes = encode_commit_metadata(&metadata).expect("metadata encodes");

        for forbidden in [
            b"sha256:evolution".as_slice(),
            b"sha256:proposal".as_slice(),
            b"sha256:default".as_slice(),
            b"sha256:retire".as_slice(),
        ] {
            assert!(
                !bytes
                    .windows(forbidden.len())
                    .any(|window| window == forbidden),
                "commit metadata must not persist activation receipt payloads"
            );
        }
    }
}
