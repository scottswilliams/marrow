//! Opaque backup cells for the tree-cell store.

use std::io::{Read, Write};

use crate::backend::StoreError;
use crate::cell::{
    CatalogId, DataCellKey, DataCellKind, DataPathSegment, NODE_MARKER, SequencePosition,
    decode_data_cell_key,
};
use crate::codec::BoundedReader;
use crate::key::{SavedKey, decode_key_value, encode_key_value};

const CHECKSUM_PRIME: u64 = 0x0000_0100_0000_01b3;

const TARGET_VERSION_V0: u8 = 0;
const KIND_NODE: u8 = 0;
const KIND_LEAF: u8 = 1;
const KIND_SEQUENCE: u8 = 2;
const KIND_VALUE: u8 = 3;
const SEGMENT_MEMBER: u8 = 0;
const SEGMENT_KEY: u8 = 1;
const MIN_KEY_FRAME_BYTES: usize = 6; // 4-byte chunk length + shortest bool key.
const MIN_PATH_SEGMENT_FRAME_BYTES: usize = 7; // 1-byte tag + shortest key chunk.

/// Identifies a Marrow backup archive before the manifest and typed cell stream.
pub const TREE_BACKUP_ARCHIVE_MAGIC: &[u8; 8] = b"MARROWBK";

/// The archive framing version shared by backup writers, restore, and read-only
/// preview readers.
pub const TREE_BACKUP_ARCHIVE_FORMAT_VERSION: u32 = 5;

pub const TREE_BACKUP_MAX_MANIFEST_BYTES: u32 = 16 * 1024 * 1024;
pub const TREE_BACKUP_MAX_CATALOG_SECTION_BYTES: u32 = 64 * 1024 * 1024;
pub const TREE_BACKUP_MAX_CELL_BYTES: u32 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeBackupArchiveReadError {
    NotBackupFile,
    HeaderTruncated,
    UnsupportedVersion { found: u32, expected: u32 },
    ChunkTooLarge { label: &'static str },
    ChunkTruncated { label: &'static str },
}

impl std::fmt::Display for TreeBackupArchiveReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotBackupFile => f.write_str("not a Marrow backup file"),
            Self::HeaderTruncated => f.write_str("backup header is truncated"),
            Self::UnsupportedVersion { found, expected } => write!(
                f,
                "backup format version {found} is unsupported (this build writes {expected})"
            ),
            Self::ChunkTooLarge { label } => {
                write!(f, "backup {label} is implausibly large")
            }
            Self::ChunkTruncated { label } => write!(f, "backup {label} is truncated"),
        }
    }
}

impl std::error::Error for TreeBackupArchiveReadError {}

pub fn write_tree_backup_archive_header(out: &mut impl Write) -> std::io::Result<()> {
    out.write_all(TREE_BACKUP_ARCHIVE_MAGIC)?;
    out.write_all(&TREE_BACKUP_ARCHIVE_FORMAT_VERSION.to_be_bytes())
}

pub fn read_tree_backup_archive_header(
    input: &mut impl Read,
) -> Result<(), TreeBackupArchiveReadError> {
    let mut magic = [0u8; 8];
    input
        .read_exact(&mut magic)
        .map_err(|_| TreeBackupArchiveReadError::HeaderTruncated)?;
    if &magic != TREE_BACKUP_ARCHIVE_MAGIC {
        return Err(TreeBackupArchiveReadError::NotBackupFile);
    }
    let version = read_archive_u32(input)?;
    if version != TREE_BACKUP_ARCHIVE_FORMAT_VERSION {
        return Err(TreeBackupArchiveReadError::UnsupportedVersion {
            found: version,
            expected: TREE_BACKUP_ARCHIVE_FORMAT_VERSION,
        });
    }
    Ok(())
}

pub fn write_tree_backup_archive_chunk(out: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    write_chunk(out, bytes)
}

pub fn read_tree_backup_archive_chunk(
    input: &mut impl Read,
    max_len: u32,
    label: &'static str,
) -> Result<Vec<u8>, TreeBackupArchiveReadError> {
    let len = read_archive_u32(input)
        .map_err(|_| TreeBackupArchiveReadError::ChunkTruncated { label })?;
    if len > max_len {
        return Err(TreeBackupArchiveReadError::ChunkTooLarge { label });
    }
    let mut bytes = vec![0u8; len as usize];
    input
        .read_exact(&mut bytes)
        .map_err(|_| TreeBackupArchiveReadError::ChunkTruncated { label })?;
    Ok(bytes)
}

/// One borrowed data-family cell in the canonical backup stream.
#[derive(Debug, Clone)]
pub struct TreeBackupCell<'a> {
    target: DataCellKey,
    value: &'a [u8],
}

impl<'a> TreeBackupCell<'a> {
    pub(crate) fn from_raw(key: &'a [u8], value: &'a [u8]) -> Result<Self, StoreError> {
        let target = decode_and_validate(key, value)?;
        Ok(Self { target, value })
    }

    /// The typed data-cell target carried by this backup cell.
    pub fn data_key(&self) -> &DataCellKey {
        &self.target
    }

    /// The canonical typed payload bytes carried by this backup cell.
    pub fn value(&self) -> &[u8] {
        self.value
    }

    /// Fold the exact framed cell bytes into the running checksum.
    pub fn fold_checksum(&self, hash: u64) -> u64 {
        let target = encode_target_frame(&self.target);
        let hash = fold_chunk(hash, &target);
        fold_chunk(hash, self.value)
    }

    /// Write the length-prefixed typed cell frame.
    pub fn write_framed(&self, out: &mut impl Write) -> std::io::Result<()> {
        let target = encode_target_frame(&self.target);
        write_chunk(out, &target)?;
        write_chunk(out, self.value)
    }
}

/// An owned backup cell read from an archive and validated against the tree-cell
/// typed backup grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeBackupCellBuf {
    target: DataCellKey,
    value: Vec<u8>,
}

impl TreeBackupCellBuf {
    #[cfg(test)]
    pub(crate) fn from_cell(cell: TreeBackupCell<'_>) -> Self {
        Self {
            target: cell.target,
            value: cell.value.to_vec(),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_raw(key: Vec<u8>, value: Vec<u8>) -> Result<Self, StoreError> {
        let target = decode_and_validate(&key, &value)?;
        Ok(Self { target, value })
    }

    /// Read one framed typed backup cell and validate it before restore can use it.
    pub fn read_framed(
        input: &mut impl Read,
        max_cell_bytes: u32,
    ) -> Result<Self, TreeBackupCellReadError> {
        let target = read_chunk(input, max_cell_bytes)?;
        let target = decode_target_frame(&target)?;
        let value = read_chunk(input, max_cell_bytes)?;
        validate_target_value(&target, &value).map_err(malformed)?;
        Ok(Self { target, value })
    }

    /// Read one framed typed backup cell, returning `None` only when the stream
    /// is already at a clean cell boundary.
    pub fn read_framed_optional(
        input: &mut impl Read,
        max_cell_bytes: u32,
    ) -> Result<Option<Self>, TreeBackupCellReadError> {
        let Some(target) = read_chunk_optional(input, max_cell_bytes)? else {
            return Ok(None);
        };
        let target = decode_target_frame(&target)?;
        let value = read_chunk(input, max_cell_bytes)?;
        validate_target_value(&target, &value).map_err(malformed)?;
        Ok(Some(Self { target, value }))
    }

    /// Borrow this owned cell as an opaque backup-stream cell.
    pub fn as_ref(&self) -> TreeBackupCell<'_> {
        TreeBackupCell {
            target: self.target.clone(),
            value: &self.value,
        }
    }

    /// The typed data-cell target carried by this backup cell.
    pub fn data_key(&self) -> &DataCellKey {
        &self.target
    }

    /// The canonical typed payload bytes carried by this backup cell.
    pub fn value(&self) -> &[u8] {
        &self.value
    }
}

/// Why a framed backup cell could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeBackupCellReadError {
    EndedEarly,
    CellTooLarge,
    MalformedCell,
}

impl std::fmt::Display for TreeBackupCellReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EndedEarly => f.write_str("backup cell stream ended early"),
            Self::CellTooLarge => f.write_str("backup cell is implausibly large"),
            Self::MalformedCell => f.write_str("backup cell target is malformed"),
        }
    }
}

impl std::error::Error for TreeBackupCellReadError {}

fn malformed<E>(_: E) -> TreeBackupCellReadError {
    TreeBackupCellReadError::MalformedCell
}

fn malformed_frame(_: &[u8]) -> TreeBackupCellReadError {
    TreeBackupCellReadError::MalformedCell
}

/// Single owner of the from-store decode-and-validate step: failures here mean
/// the persisted bytes are corrupt, not merely an early end of a backup stream.
fn decode_and_validate(key: &[u8], value: &[u8]) -> Result<DataCellKey, StoreError> {
    let target = decode_data_cell_key(key).ok_or_else(|| StoreError::Corruption {
        message: "backup cell key is not a well-formed data cell".into(),
    })?;
    validate_target_value(&target, value).map_err(|message| StoreError::Corruption {
        message: message.to_string(),
    })?;
    Ok(target)
}

fn validate_target_value(target: &DataCellKey, value: &[u8]) -> Result<(), &'static str> {
    if matches!(target.kind, DataCellKind::Node) && value != NODE_MARKER {
        return Err("backup node cell has a malformed marker");
    }
    Ok(())
}

fn encode_target_frame(target: &DataCellKey) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(TARGET_VERSION_V0);
    match &target.kind {
        DataCellKind::Node => out.push(KIND_NODE),
        DataCellKind::Leaf { member } => {
            out.push(KIND_LEAF);
            encode_catalog_id(member, &mut out);
        }
        DataCellKind::Sequence { member, position } => {
            out.push(KIND_SEQUENCE);
            encode_catalog_id(member, &mut out);
            out.extend_from_slice(&position.get().to_be_bytes());
        }
        DataCellKind::Value { path } => {
            out.push(KIND_VALUE);
            encode_path(path, &mut out);
        }
    }
    encode_catalog_id(&target.store, &mut out);
    encode_keys(&target.identity, &mut out);
    out
}

fn decode_target_frame(bytes: &[u8]) -> Result<DataCellKey, TreeBackupCellReadError> {
    let mut frame = BoundedReader::new(bytes, malformed_frame);
    if frame.take_u8()? != TARGET_VERSION_V0 {
        return Err(TreeBackupCellReadError::MalformedCell);
    }
    let kind = decode_kind(&mut frame)?;
    let store = read_catalog_id(&mut frame)?;
    let identity = read_keys(&mut frame)?;
    if !frame.is_empty() {
        return Err(TreeBackupCellReadError::MalformedCell);
    }
    Ok(DataCellKey {
        store,
        identity,
        kind,
    })
}

type FrameReader<'a> = BoundedReader<'a, TreeBackupCellReadError>;

fn decode_kind(frame: &mut FrameReader<'_>) -> Result<DataCellKind, TreeBackupCellReadError> {
    match frame.take_u8()? {
        KIND_NODE => Ok(DataCellKind::Node),
        KIND_LEAF => Ok(DataCellKind::Leaf {
            member: read_catalog_id(frame)?,
        }),
        KIND_SEQUENCE => Ok(DataCellKind::Sequence {
            member: read_catalog_id(frame)?,
            position: SequencePosition::new(frame.take_u64()?),
        }),
        KIND_VALUE => {
            let path = read_path(frame)?;
            if path.is_empty() {
                return Err(TreeBackupCellReadError::MalformedCell);
            }
            Ok(DataCellKind::Value { path })
        }
        _ => Err(TreeBackupCellReadError::MalformedCell),
    }
}

fn encode_catalog_id(id: &CatalogId, out: &mut Vec<u8>) {
    encode_frame_chunk(out, id.as_str().as_bytes());
}

fn encode_keys(keys: &[SavedKey], out: &mut Vec<u8>) {
    out.extend_from_slice(&(keys.len() as u32).to_be_bytes());
    for key in keys {
        encode_frame_chunk(out, &encode_key_value(key));
    }
}

fn encode_path(path: &[DataPathSegment], out: &mut Vec<u8>) {
    out.extend_from_slice(&(path.len() as u32).to_be_bytes());
    for segment in path {
        match segment {
            DataPathSegment::Member(member) => {
                out.push(SEGMENT_MEMBER);
                encode_catalog_id(member, out);
            }
            DataPathSegment::Key(key) => {
                out.push(SEGMENT_KEY);
                encode_frame_chunk(out, &encode_key_value(key));
            }
        }
    }
}

fn encode_frame_chunk(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn read_catalog_id(frame: &mut FrameReader<'_>) -> Result<CatalogId, TreeBackupCellReadError> {
    let bytes = frame.take_prefixed_bytes()?;
    let text = std::str::from_utf8(bytes).map_err(malformed)?;
    CatalogId::new(text.to_string()).map_err(malformed)
}

fn read_key(frame: &mut FrameReader<'_>) -> Result<SavedKey, TreeBackupCellReadError> {
    let bytes = frame.take_prefixed_bytes()?;
    let (key, used) = decode_key_value(bytes).ok_or(TreeBackupCellReadError::MalformedCell)?;
    if used != bytes.len() {
        return Err(TreeBackupCellReadError::MalformedCell);
    }
    Ok(key)
}

fn read_keys(frame: &mut FrameReader<'_>) -> Result<Vec<SavedKey>, TreeBackupCellReadError> {
    let len = frame.take_bounded_count(MIN_KEY_FRAME_BYTES)?;
    (0..len).map(|_| read_key(frame)).collect()
}

fn read_path(frame: &mut FrameReader<'_>) -> Result<Vec<DataPathSegment>, TreeBackupCellReadError> {
    let len = frame.take_bounded_count(MIN_PATH_SEGMENT_FRAME_BYTES)?;
    (0..len)
        .map(|_| match frame.take_u8()? {
            SEGMENT_MEMBER => Ok(DataPathSegment::Member(read_catalog_id(frame)?)),
            SEGMENT_KEY => Ok(DataPathSegment::Key(read_key(frame)?)),
            _ => Err(TreeBackupCellReadError::MalformedCell),
        })
        .collect()
}

fn fold_chunk(hash: u64, bytes: &[u8]) -> u64 {
    let hash = fold(hash, &(bytes.len() as u32).to_be_bytes());
    fold(hash, bytes)
}

fn fold(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(CHECKSUM_PRIME);
    }
    hash
}

fn write_chunk(out: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    let len = u32::try_from(bytes.len()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "backup cell chunk exceeds u32 length",
        )
    })?;
    out.write_all(&len.to_be_bytes())?;
    out.write_all(bytes)
}

fn read_chunk(
    input: &mut impl Read,
    max_cell_bytes: u32,
) -> Result<Vec<u8>, TreeBackupCellReadError> {
    let len = read_u32(input)?;
    if len > max_cell_bytes {
        return Err(TreeBackupCellReadError::CellTooLarge);
    }
    let mut bytes = vec![0u8; len as usize];
    input
        .read_exact(&mut bytes)
        .map_err(|_| TreeBackupCellReadError::EndedEarly)?;
    Ok(bytes)
}

fn read_chunk_optional(
    input: &mut impl Read,
    max_cell_bytes: u32,
) -> Result<Option<Vec<u8>>, TreeBackupCellReadError> {
    let Some(len) = read_u32_optional(input)? else {
        return Ok(None);
    };
    if len > max_cell_bytes {
        return Err(TreeBackupCellReadError::CellTooLarge);
    }
    let mut bytes = vec![0u8; len as usize];
    input
        .read_exact(&mut bytes)
        .map_err(|_| TreeBackupCellReadError::EndedEarly)?;
    Ok(Some(bytes))
}

fn read_u32(input: &mut impl Read) -> Result<u32, TreeBackupCellReadError> {
    let mut bytes = [0u8; 4];
    input
        .read_exact(&mut bytes)
        .map_err(|_| TreeBackupCellReadError::EndedEarly)?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_u32_optional(input: &mut impl Read) -> Result<Option<u32>, TreeBackupCellReadError> {
    let mut bytes = [0u8; 4];
    let mut read = 0usize;
    while read < bytes.len() {
        match input.read(&mut bytes[read..]) {
            Ok(0) if read == 0 => return Ok(None),
            Ok(0) => return Err(TreeBackupCellReadError::EndedEarly),
            Ok(n) => read += n,
            Err(_) => return Err(TreeBackupCellReadError::EndedEarly),
        }
    }
    Ok(Some(u32::from_be_bytes(bytes)))
}

fn read_archive_u32(input: &mut impl Read) -> Result<u32, TreeBackupArchiveReadError> {
    let mut bytes = [0u8; 4];
    input
        .read_exact(&mut bytes)
        .map_err(|_| TreeBackupArchiveReadError::HeaderTruncated)?;
    Ok(u32::from_be_bytes(bytes))
}
