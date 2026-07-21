//! The minimal backup/restore slice (F02b).
//!
//! This is an **explicitly disposable, non-canonical** logical-cell copy — *not* the F04
//! logical archive. It round-trips one populated store's logical content through the native
//! path so the persistent lifecycle has a working backup/restore vertical before F04, but it
//! carries **no digest claim** and is not a compatibility surface: the canonical, versioned,
//! engine-neutral, streaming archive grammar and its KAT freeze remain F04's to define (FR01
//! §1 R4 / §2). Because this slice writes no archive-shaped canonical bytes, the FR01
//! advisory-3 freeze does **not** transfer here, and the store head's reserved `data_digest`
//! slots stay zero (an unsequenced store carries no digest claim, FR01 §2).
//!
//! The slice copies the kernel's own id-keyed **logical cells** (through the closed
//! [`DurableStore::visit_cells`]/[`DurableStore::insert_cells`] maintenance seam), never
//! engine pages, so it is a logical backup, not an engine-page copy. Production and
//! consumption are streamed in bounded batches, so neither half materializes the whole store.
//! The disposable stream is `b"MWSLICE0"`, then the length-prefixed envelope and head bytes
//! copied verbatim, then length-prefixed `(key, value)` cells to end-of-stream.

use std::io::{Read, Write};
use std::path::Path;

use marrow_kernel::durable::{NativeStore, SiteSpec, StoreError, StoreSchema};

use crate::durable_fs::{sync_dir, write_file};
use crate::provision::OpenStore;
use crate::store_dir;

/// One logical cell: `(key, value)`. Matches the engine's `Cell` alias the kernel maintenance
/// seam consumes, without a direct engine dependency.
type Cell = (Vec<u8>, Vec<u8>);

/// The disposable, non-canonical slice marker. Not a versioned envelope and not a digest —
/// this is deliberately not a durability contract.
const SLICE_MAGIC: &[u8; 8] = b"MWSLICE0";

/// Restore inserts cells in bounded batches so consumption never materializes the whole
/// store; production is already bounded by the engine's scan-page contract.
const RESTORE_BATCH: usize = 256;

/// Why a slice backup or restore failed.
#[derive(Debug)]
pub enum SliceError {
    /// The stream was not a well-formed slice (bad marker, truncated frame, or a length past
    /// the bound).
    Malformed,
    /// A filesystem or stream I/O error.
    Io(std::io::Error),
    /// The ordered-byte engine failed while reading or rebuilding the store.
    Store(StoreError),
}

impl std::fmt::Display for SliceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SliceError::Malformed => write!(f, "the backup slice is malformed"),
            SliceError::Io(error) => write!(f, "backup slice I/O failed: {error}"),
            SliceError::Store(error) => write!(f, "the store engine failed: {error}"),
        }
    }
}

impl std::error::Error for SliceError {}

/// The most bytes a single length-prefixed frame (envelope, head, one key, or one value) may
/// claim — a bound before allocation (law 9). Comfortably above the store head/envelope and
/// the engine's per-cell ceilings, and far below anything that would exhaust memory.
const MAX_FRAME_BYTES: u32 = 16 * 1024 * 1024;

/// Write a disposable backup slice of the already-open `store` to `out`. The envelope and head
/// are copied verbatim; every logical cell is streamed in bounded pages.
pub fn backup_slice(store: &OpenStore, out: &mut impl Write) -> Result<(), SliceError> {
    out.write_all(SLICE_MAGIC).map_err(SliceError::Io)?;
    write_frame(out, &store.envelope.encode())?;
    write_frame(out, &store.head.encode())?;
    let mut sink_error: Option<SliceError> = None;
    let result = store.store.visit_cells(|page| {
        for (key, value) in page {
            if let Err(error) = write_frame(out, key).and_then(|()| write_frame(out, value)) {
                sink_error = Some(error);
                // Surface as a store-side stop; the real cause is carried out of band.
                return Err(StoreError::Io {
                    op: "backup_slice.write",
                    message: "the slice sink failed".to_string(),
                });
            }
        }
        Ok(())
    });
    if let Some(error) = sink_error {
        return Err(error);
    }
    result.map_err(SliceError::Store)?;
    out.flush().map_err(SliceError::Io)
}

/// Restore a disposable backup slice from `input` into a fresh store at `dest`, opened under
/// `schemas`/`sites`. Builds the store directory complete-or-not-at-all in a private sibling
/// temporary directory and atomically renames it into place, exactly as provision does, so an
/// interrupted restore never leaves a partial store at `dest`.
pub fn restore_slice(
    input: &mut impl Read,
    dest: &Path,
    schemas: Vec<StoreSchema>,
    sites: Vec<SiteSpec>,
) -> Result<(), SliceError> {
    let mut magic = [0u8; 8];
    input.read_exact(&mut magic).map_err(SliceError::Io)?;
    if &magic != SLICE_MAGIC {
        return Err(SliceError::Malformed);
    }
    let envelope = read_frame(input)?;
    let head = read_frame(input)?;

    let temp = crate::provision::temp_sibling(dest);
    match build_restored(&temp, input, schemas, sites, &envelope, &head) {
        Ok(()) => {}
        Err(error) => {
            let _ = std::fs::remove_dir_all(&temp);
            return Err(error);
        }
    }
    match std::fs::rename(&temp, dest) {
        Ok(()) => {}
        Err(error) => {
            let _ = std::fs::remove_dir_all(&temp);
            return Err(SliceError::Io(error));
        }
    }
    if let Some(parent) = dest.parent() {
        sync_dir(parent).map_err(SliceError::Io)?;
    }
    Ok(())
}

fn build_restored(
    temp: &Path,
    input: &mut impl Read,
    schemas: Vec<StoreSchema>,
    sites: Vec<SiteSpec>,
    envelope: &[u8],
    head: &[u8],
) -> Result<(), SliceError> {
    crate::provision::create_private_dir(temp).map_err(SliceError::Io)?;
    let mut store = NativeStore::open_native(&store_dir::engine_path(temp), schemas, sites)
        .map_err(SliceError::Store)?;

    // Stream the cells back in bounded batches: read up to RESTORE_BATCH cells, insert them in
    // one transaction, repeat until the stream ends.
    let mut batch: Vec<Cell> = Vec::with_capacity(RESTORE_BATCH);
    while let Some(cell) = read_cell(input)? {
        batch.push(cell);
        if batch.len() == RESTORE_BATCH {
            store.insert_cells(&batch).map_err(SliceError::Store)?;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        store.insert_cells(&batch).map_err(SliceError::Store)?;
    }
    drop(store);

    write_file(&store_dir::envelope_path(temp), envelope).map_err(SliceError::Io)?;
    write_file(&store_dir::head_path(temp), head).map_err(SliceError::Io)?;
    sync_dir(temp).map_err(SliceError::Io)
}

fn write_frame(out: &mut impl Write, bytes: &[u8]) -> Result<(), SliceError> {
    let len: u32 = bytes.len().try_into().map_err(|_| SliceError::Malformed)?;
    out.write_all(&len.to_be_bytes()).map_err(SliceError::Io)?;
    out.write_all(bytes).map_err(SliceError::Io)
}

fn read_frame(input: &mut impl Read) -> Result<Vec<u8>, SliceError> {
    read_frame_opt(input)?.ok_or(SliceError::Malformed)
}

/// Read one length-prefixed frame, or `None` at a clean end of stream (used to detect the end
/// of the cell list). A partial length prefix or a length past [`MAX_FRAME_BYTES`] is
/// malformed.
fn read_frame_opt(input: &mut impl Read) -> Result<Option<Vec<u8>>, SliceError> {
    let mut len_bytes = [0u8; 4];
    match input.read_exact(&mut len_bytes) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(SliceError::Io(error)),
    }
    let len = u32::from_be_bytes(len_bytes);
    if len > MAX_FRAME_BYTES {
        return Err(SliceError::Malformed);
    }
    let mut buf = vec![0u8; len as usize];
    input.read_exact(&mut buf).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            SliceError::Malformed
        } else {
            SliceError::Io(error)
        }
    })?;
    Ok(Some(buf))
}

/// Read one `(key, value)` cell, or `None` at end of stream. A key present without its value
/// is a truncated slice.
fn read_cell(input: &mut impl Read) -> Result<Option<Cell>, SliceError> {
    let Some(key) = read_frame_opt(input)? else {
        return Ok(None);
    };
    let value = read_frame(input)?;
    Ok(Some((key, value)))
}
