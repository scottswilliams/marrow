//! The bounded five-kind pending-journal frame: one byte layout, one decoder.
//!
//! ```text
//! magic[8] = "MWPEND0\0"
//! u8 version = 0
//! u8 kind       # 1 ids, 2 provision, 3 rebind, 4 lineage, 5 cache
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
/// Fixed prefix: magic, version, kind, reserved, `header_len`.
pub(crate) const PREFIX_LEN: usize = 16;
/// Bytes a record occupies beyond its payload: leading length, sequence, tag,
/// trailing length echo.
pub(crate) const RECORD_OVERHEAD: usize = 13;
/// The `record_len` field value for an empty payload: sequence plus tag.
pub(crate) const RECORD_LEN_BASE: u32 = 5;
/// The shared leading header of kinds 4 and 5: generation, parent identity,
/// journal-inode identity.
pub(crate) const JOURNAL_COMMON_LEN: usize = 48;

/// The five pending-journal kinds. The numeric code is the frame's `kind`
/// byte and is frozen by the known-answer tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JournalKind {
    /// Kind 1: identity-ledger publication.
    Ids,
    /// Kind 2: store provision.
    Provision,
    /// Kind 3: store rebind.
    Rebind,
    /// Kind 4: package-lineage publication.
    Lineage,
    /// Kind 5: package-cache publication.
    Cache,
}

impl JournalKind {
    /// The frame `kind` byte.
    pub const fn code(self) -> u8 {
        match self {
            Self::Ids => 1,
            Self::Provision => 2,
            Self::Rebind => 3,
            Self::Lineage => 4,
            Self::Cache => 5,
        }
    }

    /// The kind for a frame `kind` byte, if it names one.
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Ids),
            2 => Some(Self::Provision),
            3 => Some(Self::Rebind),
            4 => Some(Self::Lineage),
            5 => Some(Self::Cache),
            _ => None,
        }
    }

    /// The kind's total frame ceiling in bytes. The decoder reads at most
    /// `ceiling + 1` bytes and refuses the surplus byte before any
    /// length-derived allocation.
    pub const fn ceiling(self) -> usize {
        match self {
            Self::Ids => 2_101_248,
            Self::Provision | Self::Rebind | Self::Lineage | Self::Cache => 4_096,
        }
    }

    /// The number of phases in the kind's registry. Phase tags run `1..=n`;
    /// tag 1 is `Prepared` and tag `n` is the terminal phase.
    pub const fn phase_count(self) -> u8 {
        match self {
            Self::Ids | Self::Provision | Self::Lineage => 3,
            Self::Rebind => 6,
            Self::Cache => 5,
        }
    }

    /// Whether `phase_tag` is the kind's terminal phase. This is the one
    /// statement of what completeness means; every holder of a last tag asks
    /// here rather than comparing against the registry itself.
    pub const fn is_terminal(self, phase_tag: u8) -> bool {
        phase_tag == self.phase_count()
    }

    /// The exact `header_len` for kinds whose row header is closed (4 and 5).
    /// Kinds 1–3 carry their consumer rows' headers, bounded by the ceiling.
    pub const fn exact_header_len(self) -> Option<usize> {
        match self {
            Self::Lineage => Some(184),
            Self::Cache => Some(285),
            Self::Ids | Self::Provision | Self::Rebind => None,
        }
    }

    /// Whether the kind's row header begins with a leading [`JournalCommon`]
    /// self-witness (kinds 4 and 5).
    pub const fn carries_self_witness(self) -> bool {
        matches!(self, Self::Lineage | Self::Cache)
    }

    /// The exact phase-payload length by record position for kinds whose
    /// record sizes are closed (4 and 5).
    pub const fn exact_payload_len(self, position: u32) -> Option<usize> {
        match self {
            Self::Lineage => match position {
                0 => Some(1),
                1 | 2 => Some(33),
                _ => None,
            },
            Self::Cache => match position {
                0 => Some(1),
                1 => Some(32),
                2 => Some(128),
                3 => Some(105),
                4 => Some(81),
                _ => None,
            },
            Self::Ids | Self::Provision | Self::Rebind => None,
        }
    }
}

impl fmt::Display for JournalKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Ids => "ids",
            Self::Provision => "provision",
            Self::Rebind => "rebind",
            Self::Lineage => "lineage",
            Self::Cache => "cache",
        };
        formatter.write_str(name)
    }
}

/// The shared leading header of kinds 4 and 5. Generation is header evidence
/// only, never a name, semantic identity, or cleanup wildcard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalCommon {
    /// The row's 16-byte generation evidence (`PackageLineage[0..16]` for
    /// kind 4, `PackageRecordId[0..16]` for kind 5).
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
    /// The record's dense monotone sequence, starting at zero.
    pub fn sequence(&self) -> u32 {
        self.sequence
    }

    /// The record's phase tag within the kind's registry.
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
    pub(crate) kind: JournalKind,
    pub(crate) row_header: Vec<u8>,
    pub(crate) records: Vec<PhaseRecord>,
    pub(crate) tail: TailState,
}

impl DecodedFrame {
    /// The frame's kind.
    pub fn kind(&self) -> JournalKind {
        self.kind
    }

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

    /// The last recorded phase tag; zero before the first record.
    pub fn last_tag(&self) -> u8 {
        self.records.last().map_or(0, |record| record.phase_tag)
    }

    /// Whether the final registry phase has been recorded.
    pub fn is_complete(&self) -> bool {
        self.kind.is_terminal(self.last_tag())
    }

    /// The `JournalCommon` leading kinds 4 and 5; `None` for kinds 1–3.
    pub fn journal_common(&self) -> Option<JournalCommon> {
        if !self.kind.carries_self_witness() {
            return None;
        }
        let common: &[u8; JOURNAL_COMMON_LEN] = self.row_header[..JOURNAL_COMMON_LEN]
            .try_into()
            .expect("a closed-kind header begins with the 48-byte common");
        Some(JournalCommon::decode(common))
    }
}

/// The record law: the rules every record satisfies at its position in the
/// frame, stated once. The encoder checks them before it writes; the decoder
/// checks the same rules against the bytes it finds. Neither direction
/// restates one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordLaw {
    /// The position is beyond the kind's registry: each phase appears at most
    /// once, so a frame holds at most `phase_count` records.
    SequenceOutOfRegistry,
    /// The phase tag is outside the kind's registry `1..=n`.
    TagOutOfRegistry { found: u8 },
    /// The first record's phase is always `Prepared`.
    FirstTagNotPrepared { found: u8 },
    /// The phase tag does not strictly advance past the preceding record's.
    TagNotAdvancing { previous: u8, found: u8 },
    /// A closed-record kind requires dense phase tags equal to position + 1.
    TagNotDense { found: u8 },
    /// A closed-record kind requires the exact payload length for the position.
    WrongPayloadLength { expected: usize, found: usize },
    /// The record does not end under the kind's ceiling at this position.
    OverCeiling { ceiling: usize, end: usize },
}

impl fmt::Display for RecordLaw {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SequenceOutOfRegistry => formatter.write_str("is beyond the kind's registry"),
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
            Self::TagNotDense { found } => write!(
                formatter,
                "carries phase tag {found}, breaking this kind's dense registry"
            ),
            Self::WrongPayloadLength { expected, found } => write!(
                formatter,
                "carries {found} payload bytes, not the exact {expected}"
            ),
            Self::OverCeiling { ceiling, end } => {
                write!(
                    formatter,
                    "ends at byte {end}, past the {ceiling}-byte ceiling"
                )
            }
        }
    }
}

impl JournalKind {
    /// The half of the record law a declared length settles: registry
    /// position, exact payload length, and ceiling fit for a record beginning
    /// at byte `start`.
    pub(crate) fn check_record_size(
        self,
        sequence: u32,
        payload_len: usize,
        start: usize,
    ) -> Result<(), RecordLaw> {
        if sequence >= u32::from(self.phase_count()) {
            return Err(RecordLaw::SequenceOutOfRegistry);
        }
        if let Some(expected) = self.exact_payload_len(sequence)
            && payload_len != expected
        {
            return Err(RecordLaw::WrongPayloadLength {
                expected,
                found: payload_len,
            });
        }
        let end = start + RECORD_OVERHEAD + payload_len;
        if end > self.ceiling() {
            return Err(RecordLaw::OverCeiling {
                ceiling: self.ceiling(),
                end,
            });
        }
        Ok(())
    }

    /// The half of the record law a phase tag settles, against the preceding
    /// record's tag (zero before the first record).
    pub(crate) fn check_record_tag(
        self,
        sequence: u32,
        previous: u8,
        phase_tag: u8,
    ) -> Result<(), RecordLaw> {
        if phase_tag == 0 || phase_tag > self.phase_count() {
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
        if self.exact_payload_len(sequence).is_some() && u32::from(phase_tag) != sequence + 1 {
            return Err(RecordLaw::TagNotDense { found: phase_tag });
        }
        Ok(())
    }
}

/// Check a whole record against its kind's law: the one gate a producer
/// passes, and the two halves the decoder reaches as the bytes arrive.
pub(crate) fn check_record_law(
    kind: JournalKind,
    sequence: u32,
    previous: u8,
    phase_tag: u8,
    payload_len: usize,
    start: usize,
) -> Result<(), RecordLaw> {
    kind.check_record_size(sequence, payload_len, start)?;
    kind.check_record_tag(sequence, previous, phase_tag)
}

/// A producer-side refusal: the requested header or record violates the
/// kind's frame law and was never encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameLawError {
    /// A closed-header kind was given a row header of the wrong exact length.
    WrongHeaderLength {
        kind: JournalKind,
        expected: usize,
        found: usize,
    },
    /// The row header leaves no room for the Prepared record under the
    /// kind's ceiling.
    HeaderOverCeiling { kind: JournalKind, found: usize },
    /// The record at `sequence` violates the kind's record law.
    Record { sequence: u32, law: RecordLaw },
}

impl fmt::Display for FrameLawError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongHeaderLength {
                kind,
                expected,
                found,
            } => write!(
                formatter,
                "a {kind} row header is exactly {expected} bytes (found {found})"
            ),
            Self::HeaderOverCeiling { kind, found } => write!(
                formatter,
                "a {found}-byte row header leaves no record room under the {kind} ceiling"
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
    /// More than the kind's ceiling is present; the surplus byte is refused
    /// before any length-derived allocation.
    Oversized { limit: usize },
    /// The bytes end before the fixed 16-byte prefix completes.
    TooShort { found: usize },
    /// The fixed magic is absent.
    BadMagic,
    /// The version byte is not the supported version.
    BadVersion { found: u8 },
    /// The kind byte names no kind.
    BadKind { found: u8 },
    /// The kind byte names a different kind than this journal.
    WrongKind {
        expected: JournalKind,
        found: JournalKind,
    },
    /// The reserved field is not zero.
    NonzeroReserved { found: u16 },
    /// The header length violates the kind's law.
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
    /// The record found at `sequence` violates the kind's record law.
    Record { sequence: u32, law: RecordLaw },
    /// Bytes follow the terminal registry phase.
    TrailingBytes { found: usize },
}

impl fmt::Display for FrameCorruption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Oversized { limit } => {
                write!(formatter, "the frame exceeds its {limit}-byte ceiling")
            }
            Self::TooShort { found } => write!(
                formatter,
                "the frame ends after {found} bytes, inside the fixed prefix"
            ),
            Self::BadMagic => formatter.write_str("the frame magic is absent"),
            Self::BadVersion { found } => {
                write!(formatter, "frame version {found} is not supported")
            }
            Self::BadKind { found } => write!(formatter, "kind byte {found} names no kind"),
            Self::WrongKind { expected, found } => write!(
                formatter,
                "the frame is a {found} journal, not the expected {expected} journal"
            ),
            Self::NonzeroReserved { found } => {
                write!(formatter, "the reserved field is {found}, not zero")
            }
            Self::BadHeaderLength { found } => {
                write!(formatter, "header length {found} violates the kind's law")
            }
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

/// Encode the fixed prefix and row header for `kind`, refusing a header that
/// violates the kind's law.
pub fn encode_header(kind: JournalKind, row_header: &[u8]) -> Result<Vec<u8>, FrameLawError> {
    match kind.exact_header_len() {
        Some(expected) => {
            if row_header.len() != expected {
                return Err(FrameLawError::WrongHeaderLength {
                    kind,
                    expected,
                    found: row_header.len(),
                });
            }
        }
        None => {
            if PREFIX_LEN + row_header.len() + RECORD_OVERHEAD > kind.ceiling() {
                return Err(FrameLawError::HeaderOverCeiling {
                    kind,
                    found: row_header.len(),
                });
            }
        }
    }
    let header_len =
        u32::try_from(row_header.len()).expect("a lawful row header fits the length field");
    let mut bytes = Vec::with_capacity(PREFIX_LEN + row_header.len());
    bytes.extend_from_slice(&MAGIC);
    bytes.push(VERSION);
    bytes.push(kind.code());
    bytes.extend_from_slice(&0u16.to_be_bytes());
    bytes.extend_from_slice(&header_len.to_be_bytes());
    bytes.extend_from_slice(row_header);
    Ok(bytes)
}

/// Encode one record for `kind`, refusing a sequence, tag, or payload that
/// violates the kind's law. A producer knows only its own record, so the
/// ceiling is measured from a minimal frame and the preceding tag from the
/// weakest value the sequence admits; the decoder measures both exactly.
pub fn encode_record(
    kind: JournalKind,
    sequence: u32,
    phase_tag: u8,
    payload: &[u8],
) -> Result<Vec<u8>, FrameLawError> {
    let previous = u8::try_from(sequence).unwrap_or(u8::MAX);
    check_record_law(
        kind,
        sequence,
        previous,
        phase_tag,
        payload.len(),
        PREFIX_LEN,
    )
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
fn decode_prefix(expected: JournalKind, bytes: &[u8]) -> Result<FramePrefix, FrameCorruption> {
    if bytes.len() < PREFIX_LEN {
        return Err(FrameCorruption::TooShort { found: bytes.len() });
    }
    if bytes[0..8] != MAGIC {
        return Err(FrameCorruption::BadMagic);
    }
    if bytes[8] != VERSION {
        return Err(FrameCorruption::BadVersion { found: bytes[8] });
    }
    let kind =
        JournalKind::from_code(bytes[9]).ok_or(FrameCorruption::BadKind { found: bytes[9] })?;
    if kind != expected {
        return Err(FrameCorruption::WrongKind {
            expected,
            found: kind,
        });
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
    let lawful = match kind.exact_header_len() {
        Some(exact) => header_len == exact,
        None => PREFIX_LEN + header_len + RECORD_OVERHEAD <= kind.ceiling(),
    };
    if !lawful {
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
    kind: JournalKind,
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
    kind.check_record_size(sequence, payload_len, start)
        .map_err(law)?;
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
        kind.check_record_tag(sequence, previous_tag, remaining[8])
            .map_err(law)?;
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

/// Decode `bytes` as an `expected`-kind frame. The caller reads at most
/// `ceiling + 1` bytes; the decoder refuses the surplus byte before any
/// length-derived allocation and fully validates every structurally visible
/// field, including the visible fields of an incomplete tail.
pub fn decode_frame(expected: JournalKind, bytes: &[u8]) -> Result<DecodedFrame, FrameCorruption> {
    if bytes.len() > expected.ceiling() {
        return Err(FrameCorruption::Oversized {
            limit: expected.ceiling(),
        });
    }
    let FramePrefix {
        row_header,
        records_at,
    } = decode_prefix(expected, bytes)?;

    let mut records: Vec<PhaseRecord> = Vec::new();
    let mut offset = records_at;
    let mut last_tag: u8 = 0;
    let tail = loop {
        if offset == bytes.len() {
            break TailState::Clean;
        }
        if expected.is_terminal(last_tag) {
            return Err(FrameCorruption::TrailingBytes {
                found: bytes.len() - offset,
            });
        }
        let sequence = u32::try_from(records.len()).expect("at most phase_count records");
        let remaining = &bytes[offset..];
        match check_visible_fields(expected, sequence, last_tag, offset, remaining)? {
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
        kind: expected,
        row_header,
        records,
        tail,
    })
}
