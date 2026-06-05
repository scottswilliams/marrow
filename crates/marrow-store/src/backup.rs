//! Opaque backup cells for the tree-cell store.

use std::io::{Read, Write};

use crate::backend::StoreError;
use crate::cell::{
    CatalogId, DataCellKey, DataCellKind, DataPathSegment, NODE_MARKER, SequencePosition,
    decode_data_cell_key,
};
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

/// One borrowed data-family cell in the canonical backup stream.
#[derive(Debug, Clone)]
pub struct TreeBackupCell<'a> {
    target: DataCellKey,
    value: &'a [u8],
}

impl<'a> TreeBackupCell<'a> {
    pub(crate) fn from_raw(key: &'a [u8], value: &'a [u8]) -> Result<Self, StoreError> {
        let target = decode_data_cell_key(key).ok_or_else(|| StoreError::Corruption {
            message: "backup cell key is not a well-formed data cell".into(),
        })?;
        validate_target_value(&target, value).map_err(|message| StoreError::Corruption {
            message: message.to_string(),
        })?;
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
        let target = decode_data_cell_key(&key).ok_or_else(|| StoreError::Corruption {
            message: "backup cell key is not a well-formed data cell".into(),
        })?;
        validate_target_value(&target, &value).map_err(|message| StoreError::Corruption {
            message: message.to_string(),
        })?;
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
        validate_target_value(&target, &value)
            .map_err(|_| TreeBackupCellReadError::MalformedCell)?;
        Ok(Self { target, value })
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
    let mut frame = FrameCursor::new(bytes);
    let version = frame.read_u8()?;
    if version != TARGET_VERSION_V0 {
        return Err(TreeBackupCellReadError::MalformedCell);
    }
    let kind_tag = frame.read_u8()?;
    let kind = match kind_tag {
        KIND_NODE => DataCellKind::Node,
        KIND_LEAF => DataCellKind::Leaf {
            member: frame.read_catalog_id()?,
        },
        KIND_SEQUENCE => {
            let member = frame.read_catalog_id()?;
            let position = SequencePosition::new(frame.read_u64()?);
            DataCellKind::Sequence { member, position }
        }
        KIND_VALUE => {
            let path = frame.read_path()?;
            if path.is_empty() {
                return Err(TreeBackupCellReadError::MalformedCell);
            }
            DataCellKind::Value { path }
        }
        _ => return Err(TreeBackupCellReadError::MalformedCell),
    };
    let store = frame.read_catalog_id()?;
    let identity = frame.read_keys()?;
    if !frame.is_empty() {
        return Err(TreeBackupCellReadError::MalformedCell);
    }
    Ok(DataCellKey {
        store,
        identity,
        kind,
    })
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

struct FrameCursor<'a> {
    bytes: &'a [u8],
}

impl<'a> FrameCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    fn read_u8(&mut self) -> Result<u8, TreeBackupCellReadError> {
        let Some((byte, rest)) = self.bytes.split_first() else {
            return Err(TreeBackupCellReadError::MalformedCell);
        };
        self.bytes = rest;
        Ok(*byte)
    }

    fn read_u32(&mut self) -> Result<u32, TreeBackupCellReadError> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes(
            bytes
                .try_into()
                .map_err(|_| TreeBackupCellReadError::MalformedCell)?,
        ))
    }

    fn read_u64(&mut self) -> Result<u64, TreeBackupCellReadError> {
        let bytes = self.take(8)?;
        Ok(u64::from_be_bytes(
            bytes
                .try_into()
                .map_err(|_| TreeBackupCellReadError::MalformedCell)?,
        ))
    }

    fn read_chunk(&mut self) -> Result<&'a [u8], TreeBackupCellReadError> {
        let len = self.read_u32()? as usize;
        self.take(len)
    }

    fn read_catalog_id(&mut self) -> Result<CatalogId, TreeBackupCellReadError> {
        let bytes = self.read_chunk()?;
        let text =
            std::str::from_utf8(bytes).map_err(|_| TreeBackupCellReadError::MalformedCell)?;
        CatalogId::new(text.to_string()).map_err(|_| TreeBackupCellReadError::MalformedCell)
    }

    fn read_key(&mut self) -> Result<SavedKey, TreeBackupCellReadError> {
        let bytes = self.read_chunk()?;
        let (key, used) = decode_key_value(bytes).ok_or(TreeBackupCellReadError::MalformedCell)?;
        if used != bytes.len() {
            return Err(TreeBackupCellReadError::MalformedCell);
        }
        Ok(key)
    }

    fn read_keys(&mut self) -> Result<Vec<SavedKey>, TreeBackupCellReadError> {
        let len = self.read_bounded_count(MIN_KEY_FRAME_BYTES)?;
        let mut keys = Vec::new();
        for _ in 0..len {
            keys.push(self.read_key()?);
        }
        Ok(keys)
    }

    fn read_path(&mut self) -> Result<Vec<DataPathSegment>, TreeBackupCellReadError> {
        let len = self.read_bounded_count(MIN_PATH_SEGMENT_FRAME_BYTES)?;
        let mut path = Vec::new();
        for _ in 0..len {
            match self.read_u8()? {
                SEGMENT_MEMBER => path.push(DataPathSegment::Member(self.read_catalog_id()?)),
                SEGMENT_KEY => path.push(DataPathSegment::Key(self.read_key()?)),
                _ => return Err(TreeBackupCellReadError::MalformedCell),
            }
        }
        Ok(path)
    }

    fn read_bounded_count(
        &mut self,
        min_element_bytes: usize,
    ) -> Result<usize, TreeBackupCellReadError> {
        let len = self.read_u32()? as usize;
        if len > self.bytes.len() / min_element_bytes {
            return Err(TreeBackupCellReadError::MalformedCell);
        }
        Ok(len)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], TreeBackupCellReadError> {
        if self.bytes.len() < len {
            return Err(TreeBackupCellReadError::MalformedCell);
        }
        let (head, rest) = self.bytes.split_at(len);
        self.bytes = rest;
        Ok(head)
    }
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

fn read_u32(input: &mut impl Read) -> Result<u32, TreeBackupCellReadError> {
    let mut bytes = [0u8; 4];
    input
        .read_exact(&mut bytes)
        .map_err(|_| TreeBackupCellReadError::EndedEarly)?;
    Ok(u32::from_be_bytes(bytes))
}
