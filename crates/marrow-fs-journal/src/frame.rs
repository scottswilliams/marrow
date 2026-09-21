//! The bounded pending-journal frame: one byte layout, one decoder.
//!
//! ```text
//! magic[8] = "MWPEND0\0"
//! u8 version = 0
//! u8 kind = 1
//! u16_be reserved = 0
//! u32_be(header_len)
//! row_specific_header
//! record*
//!
//! record =
//!     u32_be(record_len)
//!   || u32_be(monotone_sequence)
//!   || u8 phase_tag
//!   || phase_payload
//!   || u32_be(record_len)
//! ```
//!
//! `record_len = 4 + 1 + phase_payload.len`. There is no CRC, frame digest,
//! alignment, optional field, rewrite, rename, or extension. The complete
//! header and admitted artifact state determine the unique legal next record;
//! only an incomplete final record whose bytes are an exact prefix of that
//! unique record may be truncated and re-appended. Everything else is
//! corruption and authorizes no artifact mutation.

use std::fmt;

use crate::custody::FsIdentity;

/// The fixed frame magic.
pub(crate) const MAGIC: [u8; 8] = *b"MWPEND0\0";
/// The one supported frame version.
pub(crate) const VERSION: u8 = 0;
/// The frame `kind` byte, frozen by the known-answer tests: 1 names
/// identity-ledger publication, the one row that publishes through a journal.
const KIND: u8 = 1;
/// Fixed prefix: magic, version, kind, reserved, `header_len`.
pub(crate) const PREFIX_LEN: usize = 16;
/// Bytes a record occupies beyond its payload: leading length, sequence, tag,
/// trailing length echo.
pub(crate) const RECORD_OVERHEAD: usize = 13;
/// The `record_len` field value for an empty payload: sequence plus tag.
pub(crate) const RECORD_LEN_BASE: u32 = 5;
/// The length of the common `claim` composes at the head of every row header;
/// the frame law treats it as header bytes.
pub(crate) const JOURNAL_COMMON_LEN: usize = 48;
/// The total frame ceiling in bytes, sized for the identity-ledger row. The
/// decoder reads at most `CEILING + 1` bytes and refuses the surplus byte
/// before any length-derived allocation.
pub(crate) const CEILING: usize = 2_101_248;
/// The number of phases in the registry. Phase tags run `1..=PHASE_COUNT`;
/// tag 1 is `Prepared` and tag `PHASE_COUNT` is the terminal phase.
const PHASE_COUNT: u8 = 3;

/// Whether `phase_tag` is the terminal phase. This is the one statement of
/// what completeness means; every holder of a last tag asks here rather than
/// comparing against the registry itself.
pub(crate) const fn is_terminal(phase_tag: u8) -> bool {
    phase_tag == PHASE_COUNT
}

/// The common `claim` composes at the head of every row header: generation,
/// parent identity, journal-inode identity. The frame law treats it as header
/// bytes; the consumer that planned the header checks it on replay. Generation
/// is header evidence only, never a name, semantic identity, or cleanup
/// wildcard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalCommon {
    /// The row's 16-byte generation evidence.
    pub generation: [u8; 16],
    /// The admitted parent directory's identity at claim time.
    pub parent: FsIdentity,
    /// The journal file's own inode identity at claim time.
    pub journal_inode: FsIdentity,
}

impl JournalCommon {
    /// The frozen 48-byte layout: generation, parent identity, inode identity.
    pub fn encode(&self) -> [u8; JOURNAL_COMMON_LEN] {
        let mut bytes = [0u8; JOURNAL_COMMON_LEN];
        bytes[0..16].copy_from_slice(&self.generation);
        bytes[16..32].copy_from_slice(&self.parent.to_bytes());
        bytes[32..48].copy_from_slice(&self.journal_inode.to_bytes());
        bytes
    }

    /// Decode the frozen 48-byte layout.
    pub fn decode(bytes: &[u8; JOURNAL_COMMON_LEN]) -> Self {
        let field = |from: usize| -> [u8; 16] {
            bytes[from..from + 16]
                .try_into()
                .expect("a 16-byte field of the fixed layout")
        };
        Self {
            generation: field(0),
            parent: FsIdentity::from_bytes(field(16)),
            journal_inode: FsIdentity::from_bytes(field(32)),
        }
    }
}

/// One complete replayed record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseRecord {
    pub(crate) sequence: u32,
    pub(crate) phase_tag: u8,
    pub(crate) payload: Vec<u8>,
}

impl PhaseRecord {
    /// The record's phase tag within the registry.
    pub fn phase_tag(&self) -> u8 {
        self.phase_tag
    }

    /// The record's phase payload.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

/// The bytes after the last complete record of a decoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailState {
    /// The frame ends exactly at a record boundary.
    Clean,
    /// An incomplete final record follows the last complete one. Whether it is
    /// an exact prefix of the unique legal next record is decided against that
    /// record's bytes; every structurally visible field has already been
    /// checked.
    IncompletePrefix {
        /// The incomplete trailing bytes.
        bytes: Vec<u8>,
    },
}

/// A structurally valid decoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame {
    pub(crate) row_header: Vec<u8>,
    pub(crate) records: Vec<PhaseRecord>,
    pub(crate) tail: TailState,
}

impl DecodedFrame {
    /// The row-specific header bytes.
    pub fn row_header(&self) -> &[u8] {
        &self.row_header
    }

    /// The complete records, in sequence order.
    pub fn records(&self) -> &[PhaseRecord] {
        &self.records
    }

    /// The bytes after the last complete record.
    pub fn tail(&self) -> &TailState {
        &self.tail
    }
}

/// The record law: the rules every record satisfies at its position in the
/// frame, stated once. The encoder checks them before it writes; the decoder
/// checks the same rules against the bytes it finds. Neither direction
/// restates one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordLaw {
    /// The position is beyond the registry: each phase appears at most once,
    /// so a frame holds at most `PHASE_COUNT` records.
    SequenceOutOfRegistry,
    /// The phase tag is outside the registry `1..=PHASE_COUNT`.
    TagOutOfRegistry { found: u8 },
    /// The first record's phase is always `Prepared`.
    FirstTagNotPrepared { found: u8 },
    /// The phase tag does not strictly advance past the preceding record's.
    TagNotAdvancing { previous: u8, found: u8 },
    /// The record does not end under the ceiling at this position.
    OverCeiling { end: usize },
}

impl fmt::Display for RecordLaw {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SequenceOutOfRegistry => formatter.write_str("is beyond the registry"),
            Self::TagOutOfRegistry { found } => {
                write!(formatter, "carries phase tag {found}, outside the registry")
            }
            Self::FirstTagNotPrepared { found } => {
                write!(formatter, "carries phase tag {found}, not Prepared")
            }
            Self::TagNotAdvancing { previous, found } => write!(
                formatter,
                "carries phase tag {found}, which does not advance past {previous}"
            ),
            Self::OverCeiling { end } => {
                write!(
                    formatter,
                    "ends at byte {end}, past the {CEILING}-byte ceiling"
                )
            }
        }
    }
}

/// The half of the record law a declared length settles: registry position
/// and ceiling fit for a record beginning at byte `start`.
fn check_record_size(sequence: u32, payload_len: usize, start: usize) -> Result<(), RecordLaw> {
    if sequence >= u32::from(PHASE_COUNT) {
        return Err(RecordLaw::SequenceOutOfRegistry);
    }
    let end = start + RECORD_OVERHEAD + payload_len;
    if end > CEILING {
        return Err(RecordLaw::OverCeiling { end });
    }
    Ok(())
}

/// The half of the record law a phase tag settles, against the preceding
/// record's tag (zero before the first record).
fn check_record_tag(sequence: u32, previous: u8, phase_tag: u8) -> Result<(), RecordLaw> {
    if phase_tag == 0 || phase_tag > PHASE_COUNT {
        return Err(RecordLaw::TagOutOfRegistry { found: phase_tag });
    }
    if sequence == 0 && phase_tag != 1 {
        return Err(RecordLaw::FirstTagNotPrepared { found: phase_tag });
    }
    if phase_tag <= previous {
        return Err(RecordLaw::TagNotAdvancing {
            previous,
            found: phase_tag,
        });
    }
    Ok(())
}

/// A producer-side refusal: the requested header or record violates the frame
/// law and was never encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameLawError {
    /// The row header leaves no room for the Prepared record under the
    /// ceiling.
    HeaderOverCeiling { found: usize },
    /// The record at `sequence` violates the record law.
    Record { sequence: u32, law: RecordLaw },
}

impl fmt::Display for FrameLawError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HeaderOverCeiling { found } => write!(
                formatter,
                "a {found}-byte row header leaves no record room under the {CEILING}-byte ceiling"
            ),
            Self::Record { sequence, law } => {
                write!(formatter, "the record at sequence {sequence} {law}")
            }
        }
    }
}

impl std::error::Error for FrameLawError {}

/// Why a byte run is not a valid frame. Corruption authorizes no artifact
/// mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameCorruption {
    /// More than the ceiling is present; the surplus byte is refused before
    /// any length-derived allocation.
    Oversized,
    /// The bytes end before the fixed 16-byte prefix completes.
    TooShort { found: usize },
    /// The fixed magic is absent.
    BadMagic,
    /// The version byte is not the supported version.
    BadVersion { found: u8 },
    /// The kind byte is not the pending-journal kind.
    BadKind { found: u8 },
    /// The reserved field is not zero.
    NonzeroReserved { found: u16 },
    /// The header length leaves no record room under the ceiling.
    BadHeaderLength { found: u32 },
    /// The bytes end inside the row header, which the claim protocol makes
    /// durable before any link.
    HeaderTruncated { expected: usize, found: usize },
    /// A record's declared length is below the sequence-and-tag base, so no
    /// payload length can be derived from it.
    BadRecordLength { sequence: u32, found: u32 },
    /// A record's trailing length echo differs from its leading length.
    LengthEchoMismatch { sequence: u32 },
    /// The record sequence is not dense from zero.
    SequenceNotDense { expected: u32, found: u32 },
    /// The record found at `sequence` violates the record law.
    Record { sequence: u32, law: RecordLaw },
    /// Bytes follow the terminal registry phase.
    TrailingBytes { found: usize },
}

impl fmt::Display for FrameCorruption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Oversized => {
                write!(formatter, "the frame exceeds its {CEILING}-byte ceiling")
            }
            Self::TooShort { found } => write!(
                formatter,
                "the frame ends after {found} bytes, inside the fixed prefix"
            ),
            Self::BadMagic => formatter.write_str("the frame magic is absent"),
            Self::BadVersion { found } => {
                write!(formatter, "frame version {found} is not supported")
            }
            Self::BadKind { found } => {
                write!(
                    formatter,
                    "kind byte {found} is not the pending-journal kind"
                )
            }
            Self::NonzeroReserved { found } => {
                write!(formatter, "the reserved field is {found}, not zero")
            }
            Self::BadHeaderLength { found } => write!(
                formatter,
                "header length {found} leaves no record room under the ceiling"
            ),
            Self::HeaderTruncated { expected, found } => write!(
                formatter,
                "the frame ends inside its {expected}-byte row header ({found} bytes present)"
            ),
            Self::BadRecordLength { sequence, found } => write!(
                formatter,
                "the record at sequence {sequence} declares an impossible length {found}"
            ),
            Self::LengthEchoMismatch { sequence } => write!(
                formatter,
                "the record at sequence {sequence} has a mismatched trailing length echo"
            ),
            Self::SequenceNotDense { expected, found } => write!(
                formatter,
                "record sequence {found} arrived where {expected} was required"
            ),
            Self::Record { sequence, law } => {
                write!(formatter, "the record at sequence {sequence} {law}")
            }
            Self::TrailingBytes { found } => write!(
                formatter,
                "{found} bytes follow the terminal registry phase"
            ),
        }
    }
}

impl std::error::Error for FrameCorruption {}

/// Encode the fixed prefix and row header, refusing a header that leaves no
/// record room under the ceiling.
pub fn encode_header(row_header: &[u8]) -> Result<Vec<u8>, FrameLawError> {
    if PREFIX_LEN + row_header.len() + RECORD_OVERHEAD > CEILING {
        return Err(FrameLawError::HeaderOverCeiling {
            found: row_header.len(),
        });
    }
    let header_len =
        u32::try_from(row_header.len()).expect("a lawful row header fits the length field");
    let mut bytes = Vec::with_capacity(PREFIX_LEN + row_header.len());
    bytes.extend_from_slice(&MAGIC);
    bytes.push(VERSION);
    bytes.push(KIND);
    bytes.extend_from_slice(&0u16.to_be_bytes());
    bytes.extend_from_slice(&header_len.to_be_bytes());
    bytes.extend_from_slice(row_header);
    Ok(bytes)
}

/// Encode one record, refusing a sequence, tag, or payload that violates the
/// record law. A producer knows only its own record, so the ceiling is
/// measured from a minimal frame and the preceding tag from the weakest value
/// the sequence admits; the decoder measures both exactly.
pub fn encode_record(
    sequence: u32,
    phase_tag: u8,
    payload: &[u8],
) -> Result<Vec<u8>, FrameLawError> {
    let previous = u8::try_from(sequence).unwrap_or(u8::MAX);
    check_record_size(sequence, payload.len(), PREFIX_LEN)
        .and_then(|()| check_record_tag(sequence, previous, phase_tag))
        .map_err(|law| FrameLawError::Record { sequence, law })?;
    let record_len =
        RECORD_LEN_BASE + u32::try_from(payload.len()).expect("a lawful payload fits the ceiling");
    let mut bytes = Vec::with_capacity(RECORD_OVERHEAD + payload.len());
    bytes.extend_from_slice(&record_len.to_be_bytes());
    bytes.extend_from_slice(&sequence.to_be_bytes());
    bytes.push(phase_tag);
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(&record_len.to_be_bytes());
    Ok(bytes)
}

/// The decoded fixed prefix and row header, and where the records begin.
struct FramePrefix {
    row_header: Vec<u8>,
    records_at: usize,
}

/// Decode and validate the fixed 16-byte prefix and the row header that
/// follows it.
fn decode_prefix(bytes: &[u8]) -> Result<FramePrefix, FrameCorruption> {
    if bytes.len() < PREFIX_LEN {
        return Err(FrameCorruption::TooShort { found: bytes.len() });
    }
    if bytes[0..8] != MAGIC {
        return Err(FrameCorruption::BadMagic);
    }
    if bytes[8] != VERSION {
        return Err(FrameCorruption::BadVersion { found: bytes[8] });
    }
    if bytes[9] != KIND {
        return Err(FrameCorruption::BadKind { found: bytes[9] });
    }
    let reserved = u16::from_be_bytes([bytes[10], bytes[11]]);
    if reserved != 0 {
        return Err(FrameCorruption::NonzeroReserved { found: reserved });
    }
    let declared_header = u32::from_be_bytes(
        bytes[12..16]
            .try_into()
            .expect("the fixed prefix carries four header-length bytes"),
    );
    let header_len = declared_header as usize;
    if PREFIX_LEN + header_len + RECORD_OVERHEAD > CEILING {
        return Err(FrameCorruption::BadHeaderLength {
            found: declared_header,
        });
    }
    let records_at = PREFIX_LEN + header_len;
    if bytes.len() < records_at {
        return Err(FrameCorruption::HeaderTruncated {
            expected: header_len,
            found: bytes.len() - PREFIX_LEN,
        });
    }
    Ok(FramePrefix {
        row_header: bytes[PREFIX_LEN..records_at].to_vec(),
        records_at,
    })
}

/// How much of the next record `remaining` carries.
enum RecordExtent {
    /// The record is present in full and occupies `disk` bytes.
    Complete { disk: usize },
    /// The record is truncated. Every field visible in `remaining` has
    /// already been checked against the law.
    Incomplete,
}

/// Check every structurally visible field of the next record, whether or not
/// the record is complete. A truncated record is a tail candidate only once
/// the fields that are present have passed the same law a complete record
/// passes.
fn check_visible_fields(
    sequence: u32,
    previous_tag: u8,
    start: usize,
    remaining: &[u8],
) -> Result<RecordExtent, FrameCorruption> {
    let law = |law| FrameCorruption::Record { sequence, law };
    if remaining.len() < 4 {
        return Ok(RecordExtent::Incomplete);
    }
    let declared = u32::from_be_bytes(
        remaining[0..4]
            .try_into()
            .expect("four declared-length bytes"),
    );
    if declared < RECORD_LEN_BASE {
        return Err(FrameCorruption::BadRecordLength {
            sequence,
            found: declared,
        });
    }
    let payload_len = (declared - RECORD_LEN_BASE) as usize;
    check_record_size(sequence, payload_len, start).map_err(law)?;
    if remaining.len() >= 8 {
        let found = u32::from_be_bytes(remaining[4..8].try_into().expect("four sequence bytes"));
        if found != sequence {
            return Err(FrameCorruption::SequenceNotDense {
                expected: sequence,
                found,
            });
        }
    }
    if remaining.len() >= 9 {
        check_record_tag(sequence, previous_tag, remaining[8]).map_err(law)?;
    }
    let disk = RECORD_OVERHEAD + payload_len;
    if remaining.len() < disk {
        return Ok(RecordExtent::Incomplete);
    }
    let echo = u32::from_be_bytes(
        remaining[disk - 4..disk]
            .try_into()
            .expect("four echo bytes"),
    );
    if echo != declared {
        return Err(FrameCorruption::LengthEchoMismatch { sequence });
    }
    Ok(RecordExtent::Complete { disk })
}

/// Take the complete record `remaining` begins with, whose visible fields
/// [`check_visible_fields`] has already admitted.
fn take_record(sequence: u32, remaining: &[u8], disk: usize) -> PhaseRecord {
    PhaseRecord {
        sequence,
        phase_tag: remaining[8],
        payload: remaining[9..disk - 4].to_vec(),
    }
}

/// Decode `bytes` as a frame. The caller reads at most `CEILING + 1` bytes;
/// the decoder refuses the surplus byte before any length-derived allocation
/// and fully validates every structurally visible field, including the
/// visible fields of an incomplete tail.
pub(crate) fn decode_frame(bytes: &[u8]) -> Result<DecodedFrame, FrameCorruption> {
    if bytes.len() > CEILING {
        return Err(FrameCorruption::Oversized);
    }
    let FramePrefix {
        row_header,
        records_at,
    } = decode_prefix(bytes)?;

    let mut records: Vec<PhaseRecord> = Vec::new();
    let mut offset = records_at;
    let mut last_tag: u8 = 0;
    let tail = loop {
        if offset == bytes.len() {
            break TailState::Clean;
        }
        if is_terminal(last_tag) {
            return Err(FrameCorruption::TrailingBytes {
                found: bytes.len() - offset,
            });
        }
        let sequence = u32::try_from(records.len()).expect("at most PHASE_COUNT records");
        let remaining = &bytes[offset..];
        match check_visible_fields(sequence, last_tag, offset, remaining)? {
            RecordExtent::Incomplete => {
                break TailState::IncompletePrefix {
                    bytes: remaining.to_vec(),
                };
            }
            RecordExtent::Complete { disk } => {
                let record = take_record(sequence, remaining, disk);
                last_tag = record.phase_tag;
                records.push(record);
                offset += disk;
            }
        }
    };

    Ok(DecodedFrame {
        row_header,
        records,
        tail,
    })
}

#[cfg(test)]
#[path = "frame_tests.rs"]
mod tests;
