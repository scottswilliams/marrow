//! A portable backup archive of a saved tree.
//!
//! An archive is the store's whole-tree dump — the ordered (path, value) pairs
//! [`Backend::scan`](crate::backend::Backend) yields from the empty prefix —
//! framed with a small manifest. Paths and values are Marrow's canonical encoded
//! bytes (see [`crate::path`] and [`crate::value`]), so an archive restores
//! byte-for-byte into any backend and is independent of any engine's on-disk
//! files. Restore replays the records inside one transaction, so a target either
//! gains the whole archive or is left unchanged.

use std::io::{Read, Write};

use crate::backend::{Backend, StoreError};

/// The archive magic, identifying the file and guarding against restoring
/// arbitrary bytes.
const MAGIC: &[u8; 8] = b"MARROW\0A";
/// The archive format version this build writes and accepts.
const FORMAT_VERSION: u32 = 1;

fn io(op: &'static str) -> impl Fn(std::io::Error) -> StoreError {
    move |error| StoreError::Io {
        op,
        message: error.to_string(),
    }
}

/// Write `backend`'s whole saved tree to `out` as an archive, returning the
/// number of records written. The records are in Marrow order, so two archives of
/// equal data are byte-identical.
pub fn write_archive(backend: &dyn Backend, out: &mut dyn Write) -> Result<u64, StoreError> {
    let page = backend.scan(&[], usize::MAX)?;
    out.write_all(MAGIC).map_err(io("backup"))?;
    out.write_all(&FORMAT_VERSION.to_le_bytes())
        .map_err(io("backup"))?;
    let count = page.entries.len() as u64;
    out.write_all(&count.to_le_bytes()).map_err(io("backup"))?;
    for (path, value) in &page.entries {
        write_chunk(out, path)?;
        write_chunk(out, value)?;
    }
    Ok(count)
}

/// Restore an archive read from `input` into `backend`, returning the number of
/// records restored. The replay runs in one transaction: any read or write error
/// rolls the target back to its prior state. The caller decides target policy
/// (e.g. requiring an empty target for a normal restore).
pub fn read_archive(input: &mut dyn Read, backend: &mut dyn Backend) -> Result<u64, StoreError> {
    let count = read_header(input)?;
    backend.begin()?;
    match restore_records(input, backend, count) {
        Ok(()) => {
            backend.commit()?;
            Ok(count)
        }
        Err(error) => {
            backend.rollback()?;
            Err(error)
        }
    }
}

/// Read and validate the archive manifest, returning its record count.
fn read_header(input: &mut dyn Read) -> Result<u64, StoreError> {
    let mut magic = [0u8; 8];
    input.read_exact(&mut magic).map_err(io("restore"))?;
    if &magic != MAGIC {
        return Err(StoreError::Corruption {
            message: "not a Marrow archive".into(),
        });
    }
    let version = read_u32(input)?;
    if version != FORMAT_VERSION {
        return Err(StoreError::FormatVersion {
            found: version,
            supported: FORMAT_VERSION,
        });
    }
    let mut count = [0u8; 8];
    input.read_exact(&mut count).map_err(io("restore"))?;
    Ok(u64::from_le_bytes(count))
}

/// Replay `count` (path, value) records from `input` into `backend`.
fn restore_records(
    input: &mut dyn Read,
    backend: &mut dyn Backend,
    count: u64,
) -> Result<(), StoreError> {
    for _ in 0..count {
        let path = read_chunk(input)?;
        let value = read_chunk(input)?;
        backend.write(&path, value)?;
    }
    Ok(())
}

/// Write a length-prefixed byte chunk: `u32` little-endian length, then bytes.
fn write_chunk(out: &mut dyn Write, bytes: &[u8]) -> Result<(), StoreError> {
    let len = chunk_len(bytes.len())?;
    out.write_all(&len.to_le_bytes()).map_err(io("backup"))?;
    out.write_all(bytes).map_err(io("backup"))?;
    Ok(())
}

/// The `u32` length prefix for a chunk of `len` bytes. Archive framing is the sole
/// producer of [`StoreError::LimitExceeded`]: a chunk longer than `u32::MAX` would
/// not fit the length prefix, so it is a typed limit error rather than a silent
/// truncation. (No backend enforces a key/value size limit; this is the only path
/// that yields `store.limit`.)
fn chunk_len(len: usize) -> Result<u32, StoreError> {
    u32::try_from(len).map_err(|_| StoreError::LimitExceeded {
        limit: "archive chunk length",
    })
}

/// Read a length-prefixed byte chunk written by [`write_chunk`]. The chunk is read
/// up to the declared length without pre-allocating it, so a corrupt length cannot
/// force a huge allocation; a short read is a typed corruption error.
fn read_chunk(input: &mut dyn Read) -> Result<Vec<u8>, StoreError> {
    let len = u64::from(read_u32(input)?);
    let mut bytes = Vec::new();
    let read = input
        .take(len)
        .read_to_end(&mut bytes)
        .map_err(io("restore"))?;
    if read as u64 != len {
        return Err(StoreError::Corruption {
            message: "archive ended mid-record".into(),
        });
    }
    Ok(bytes)
}

fn read_u32(input: &mut dyn Read) -> Result<u32, StoreError> {
    let mut bytes = [0u8; 4];
    input.read_exact(&mut bytes).map_err(io("restore"))?;
    Ok(u32::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The limit is exercised with a faked length, not a real allocation;
    /// `bytes.len()` cannot exceed `usize::MAX`, which equals `u32::MAX` on 32-bit,
    /// so the over-length case only exists where the `checked_add` succeeds.
    #[test]
    fn an_over_length_chunk_is_a_limit_error() {
        if let Some(too_long) = (u32::MAX as usize).checked_add(1) {
            assert_eq!(
                chunk_len(too_long),
                Err(StoreError::LimitExceeded {
                    limit: "archive chunk length"
                })
            );
            assert_eq!(chunk_len(too_long).unwrap_err().code(), "store.limit");
        }
        // A length within the prefix succeeds.
        assert_eq!(chunk_len(5), Ok(5));
        assert_eq!(chunk_len(u32::MAX as usize), Ok(u32::MAX));
    }
}
