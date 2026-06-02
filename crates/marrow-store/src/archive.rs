//! Raw saved-path archive format for debug/admin access to the ordered-byte backend.
//!
//! An archive is a raw ordered (path, value) stream framed with a small manifest.
//! This module is crate-private; the public surface is [`crate::debug_admin`].

use std::io::{ErrorKind, Read, Write};

use crate::backend::{Backend, StoreError};

/// The archive magic, identifying the file and guarding against accepting
/// arbitrary bytes.
const MAGIC: &[u8; 8] = b"MARROW\0A";
/// The archive format version this build writes and accepts.
const FORMAT_VERSION: u32 = 1;
const ARCHIVE_SCAN_LIMIT: usize = 1024;

fn io(op: &'static str) -> impl Fn(std::io::Error) -> StoreError {
    move |error| StoreError::Io {
        op,
        message: error.to_string(),
    }
}

pub(crate) fn write_raw_saved_path_archive(
    backend: &dyn Backend,
    out: &mut dyn Write,
) -> Result<u64, StoreError> {
    let count = count_records(backend)?;
    out.write_all(MAGIC).map_err(io("archive.write"))?;
    out.write_all(&FORMAT_VERSION.to_le_bytes())
        .map_err(io("archive.write"))?;
    out.write_all(&count.to_le_bytes())
        .map_err(io("archive.write"))?;
    write_records(backend, out)?;
    Ok(count)
}

fn count_records(backend: &dyn Backend) -> Result<u64, StoreError> {
    let mut count = 0u64;
    scan_records(backend, |_, _| {
        count = count.checked_add(1).ok_or(StoreError::LimitExceeded {
            limit: "archive record count",
        })?;
        Ok(())
    })?;
    Ok(count)
}

fn write_records(backend: &dyn Backend, out: &mut dyn Write) -> Result<(), StoreError> {
    scan_records(backend, |path, value| {
        write_chunk(out, path)?;
        write_chunk(out, value)
    })
}

fn scan_records(
    backend: &dyn Backend,
    mut visit: impl FnMut(&[u8], &[u8]) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let mut cursor = None;
    loop {
        let page = match cursor.as_deref() {
            Some(cursor) => backend.scan_after(&[], cursor, ARCHIVE_SCAN_LIMIT)?,
            None => backend.scan(&[], ARCHIVE_SCAN_LIMIT)?,
        };
        for (path, value) in &page.entries {
            visit(path, value)?;
        }
        if !page.truncated {
            return Ok(());
        }
        cursor = page.entries.last().map(|(path, _)| path.clone());
        if cursor.is_none() {
            return Ok(());
        }
    }
}

pub(crate) fn read_raw_saved_path_archive(
    input: &mut dyn Read,
    backend: &mut dyn Backend,
) -> Result<u64, StoreError> {
    let count = read_header(input)?;
    backend.begin()?;
    let result =
        read_records_into_backend(input, backend, count).and_then(|()| require_archive_eof(input));
    match result {
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
    input.read_exact(&mut magic).map_err(io("archive.read"))?;
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
    input.read_exact(&mut count).map_err(io("archive.read"))?;
    Ok(u64::from_le_bytes(count))
}

/// Replay `count` (path, value) records from `input` into `backend`.
fn read_records_into_backend(
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

/// Require the archive body to end exactly after its declared records.
fn require_archive_eof(input: &mut dyn Read) -> Result<(), StoreError> {
    let mut trailing = [0u8; 1];
    loop {
        match input.read(&mut trailing) {
            Ok(0) => return Ok(()),
            Ok(_) => {
                return Err(StoreError::Corruption {
                    message: "archive has trailing bytes after its declared record count".into(),
                });
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(io("archive.read")(error)),
        }
    }
}

/// Write a length-prefixed byte chunk: `u32` little-endian length, then bytes.
fn write_chunk(out: &mut dyn Write, bytes: &[u8]) -> Result<(), StoreError> {
    let len = chunk_len(bytes.len())?;
    out.write_all(&len.to_le_bytes())
        .map_err(io("archive.write"))?;
    out.write_all(bytes).map_err(io("archive.write"))?;
    Ok(())
}

/// The `u32` length prefix for a chunk of `len` bytes. A chunk longer than
/// `u32::MAX` would not fit the length prefix, so it is a typed limit error
/// rather than a silent truncation. No backend enforces a key/value size limit;
/// Marrow-owned framing layers are what yield `store.limit`.
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
        .map_err(io("archive.read"))?;
    if read as u64 != len {
        return Err(StoreError::Corruption {
            message: "archive ended mid-record".into(),
        });
    }
    Ok(bytes)
}

fn read_u32(input: &mut dyn Read) -> Result<u32, StoreError> {
    let mut bytes = [0u8; 4];
    input.read_exact(&mut bytes).map_err(io("archive.read"))?;
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
