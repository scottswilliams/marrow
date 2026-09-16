//! A bounded, big-endian reader and the typed decode-rejection family the persisted
//! store artifacts (envelope, logical head) share.
//!
//! Every persisted store artifact is a versioned, length-prefixed, big-endian container
//! whose decode validates each length against the remaining input before allocating,
//! rejects an unknown version, and rejects trailing bytes — the same trust-path discipline
//! the program image obeys. A decode rejection is the "artifact decode/verify rejection"
//! failure family: distinct from an operational store error, it means the persisted bytes
//! are not a well-formed artifact this build accepts.

use marrow_codes::Code;

/// A field of a persisted store artifact's grammar, named by a decode rejection. The set is
/// closed: a rejection names a field this build's decoders actually read, and a caller
/// matches the variant rather than the spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatField {
    /// The envelope's writer-toolchain string.
    WriterToolchain,
    /// The envelope's engine-kind discriminant.
    EngineKind,
    /// The envelope's publication-state discriminant.
    PublicationState,
    /// The head's accepted-ceiling payload.
    AcceptedCeiling,
    /// The head map's entry count.
    HeadMapEntries,
    /// The head map's lifetime numbering, whose high-water bounds every entry.
    HeadMapLifetimeNumbers,
    /// A backup's embedded program image.
    BackupImage,
    /// A backup's embedded store head.
    BackupHead,
    /// A backup record's leading discriminant.
    BackupRecord,
    /// A backup cell's key block.
    BackupKey,
    /// A backup cell's value block.
    BackupValue,
    /// A backup's cell count.
    BackupCount,
}

impl FormatField {
    /// The field's name in the artifact's own vocabulary, for rendering only.
    fn name(self) -> &'static str {
        match self {
            FormatField::WriterToolchain => "writer toolchain",
            FormatField::EngineKind => "engine kind",
            FormatField::PublicationState => "publication state",
            FormatField::AcceptedCeiling => "accepted ceiling",
            FormatField::HeadMapEntries => "head map entries",
            FormatField::HeadMapLifetimeNumbers => "head map lifetime numbers",
            FormatField::BackupImage => "backup image",
            FormatField::BackupHead => "backup head",
            FormatField::BackupRecord => "backup record",
            FormatField::BackupKey => "backup key",
            FormatField::BackupValue => "backup value",
            FormatField::BackupCount => "backup count",
        }
    }
}

/// Which structural invariant of a decoded artifact is violated. Every variant is a
/// coherence rule the grammar alone cannot express, so it is checked after the fields
/// decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedReason {
    /// The head's reserved sequencing and data-digest slots are not zero.
    ReservedSlotsNotZero,
    /// The envelope's writer-toolchain bytes are not valid UTF-8.
    ToolchainNotUtf8,
    /// A head-map entry numbers at or above the map's lifetime high-water.
    HeadMapNumberAtOrAboveHighWater,
    /// Two head-map entries share a store-local number.
    HeadMapNumberReused,
    /// Two head-map entries share a ledger id.
    HeadMapLedgerIdReused,
    /// A backup's cells are not in strictly increasing key order.
    BackupCellsUnordered,
    /// A backup's trailing count disagrees with the cells that preceded it.
    BackupCountDiffers,
    /// A backup decoder was resumed after a failure, whose position it cannot recover.
    BackupInputAlreadyFailed,
}

impl MalformedReason {
    /// The violated invariant as a predicate of the artifact, for rendering only.
    fn text(self) -> &'static str {
        match self {
            MalformedReason::ReservedSlotsNotZero => {
                "the reserved sequencing and data-digest slots must be zero"
            }
            MalformedReason::ToolchainNotUtf8 => "writer toolchain is not valid UTF-8",
            MalformedReason::HeadMapNumberAtOrAboveHighWater => {
                "head map number at or above the high-water"
            }
            MalformedReason::HeadMapNumberReused => "head map reuses a number",
            MalformedReason::HeadMapLedgerIdReused => "head map reuses a ledger id",
            MalformedReason::BackupCellsUnordered => "backup cells are not strictly ordered",
            MalformedReason::BackupCountDiffers => "backup count differs",
            MalformedReason::BackupInputAlreadyFailed => "backup input previously failed",
        }
    }
}

/// Why a persisted store artifact failed to decode. Callers match the variant; the stable
/// dotted [`Code`] is for rendering only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// The artifact does not begin with its expected magic — it is not this kind of store
    /// artifact at all.
    BadMagic,
    /// The artifact records a container version this build does not read. A future version
    /// is a typed refusal, never a best-effort decode.
    UnknownVersion { found: u8 },
    /// The active binding names an image generation this build does not admit.
    UnsupportedImageVersion { found: u8 },
    /// The bytes end before a field the grammar requires — a truncated or torn artifact.
    Truncated,
    /// Bytes remain after the artifact's last field — a canonical artifact is consumed
    /// exactly, so trailing bytes are a malformed artifact, never ignored.
    TrailingBytes,
    /// A length or count field exceeds the fixed bound the grammar allows before any
    /// allocation, so a hostile length can never drive an unbounded reservation.
    LengthOverflow { field: FormatField },
    /// A discriminant or flag byte is outside the closed set the grammar defines.
    UnknownDiscriminant { field: FormatField },
    /// The recomputed digest does not match the sealed digest — the artifact's body was
    /// altered or is inconsistent with its seal.
    DigestMismatch,
    /// A structural invariant of the decoded artifact is violated (for example a head map
    /// that reuses a number or a ledger id), so the bytes are not a coherent artifact.
    Malformed { reason: MalformedReason },
}

impl FormatError {
    /// The stable dotted code a tool reports. A version this build does not read is a
    /// format-version refusal; a length beyond its bound is a representational limit;
    /// every other malformation is store corruption (the persisted bytes do not decode).
    pub fn code(&self) -> Code {
        match self {
            FormatError::UnknownVersion { .. } | FormatError::UnsupportedImageVersion { .. } => {
                Code::StoreFormatVersion
            }
            FormatError::LengthOverflow { .. } => Code::StoreLimit,
            FormatError::BadMagic
            | FormatError::Truncated
            | FormatError::TrailingBytes
            | FormatError::UnknownDiscriminant { .. }
            | FormatError::DigestMismatch
            | FormatError::Malformed { .. } => Code::StoreCorruption,
        }
    }
}

/// A rejection reads as the predicate of the artifact that carried it: the caller knows
/// which artifact it was reading and names it, so composing the two produces one sentence
/// with one subject.
impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormatError::BadMagic => {
                write!(f, "does not begin with a Marrow store artifact's magic")
            }
            FormatError::UnknownVersion { found } => {
                write!(f, "records version {found}, which this build does not read")
            }
            FormatError::UnsupportedImageVersion { found } => write!(
                f,
                "binds image generation {found}, which this build does not admit; \
                 preserve the store and use its matching toolchain for data extraction"
            ),
            FormatError::Truncated => write!(f, "is truncated"),
            FormatError::TrailingBytes => write!(f, "has trailing bytes after its last field"),
            FormatError::LengthOverflow { field } => {
                write!(f, "exceeds the bound its {} field allows", field.name())
            }
            FormatError::UnknownDiscriminant { field } => {
                write!(
                    f,
                    "has an unknown discriminant in its {} field",
                    field.name()
                )
            }
            FormatError::DigestMismatch => write!(f, "does not match its sealing digest"),
            FormatError::Malformed { reason } => write!(f, "is malformed: {}", reason.text()),
        }
    }
}

impl std::error::Error for FormatError {}

/// Every persisted store artifact opens with its 4-byte magic and a 1-byte container
/// version. An owner-held admission read classifies exactly this prefix — no more — to
/// choose the artifact's byte ceiling before it allocates for the body, so a version whose
/// framing this build has no bound for is refused ahead of any reservation.
pub(crate) const ARTIFACT_PREFIX_BYTES: usize = 5;

/// The container version `prefix` records, rejecting a prefix that is not this artifact's
/// at all. The classification shares its magic and its position with the artifact's own
/// decoder, so the ceiling an admission read applies and the verdict the decoder reaches
/// cannot drift apart.
pub(crate) fn artifact_version(
    prefix: &[u8; ARTIFACT_PREFIX_BYTES],
    magic: &[u8; 4],
) -> Result<u8, FormatError> {
    if &prefix[0..4] != magic {
        return Err(FormatError::BadMagic);
    }
    Ok(prefix[4])
}

/// A bounded forward reader over a persisted artifact's bytes. Every read validates the
/// remaining input before it borrows, so a truncated artifact rejects with
/// [`FormatError::Truncated`] rather than panicking, and [`Reader::finish`] rejects any
/// unconsumed trailing bytes.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// A reader positioned at the start of `bytes`.
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], FormatError> {
        let end = self.pos.checked_add(n).ok_or(FormatError::Truncated)?;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or(FormatError::Truncated)?;
        self.pos = end;
        Ok(slice)
    }

    /// The next byte.
    pub fn u8(&mut self) -> Result<u8, FormatError> {
        Ok(self.take(1)?[0])
    }

    /// The next big-endian `u32`.
    pub fn u32(&mut self) -> Result<u32, FormatError> {
        let raw: [u8; 4] = self.take(4)?.try_into().expect("took exactly four bytes");
        Ok(u32::from_be_bytes(raw))
    }

    /// The next big-endian `u64`.
    pub fn u64(&mut self) -> Result<u64, FormatError> {
        let raw: [u8; 8] = self.take(8)?.try_into().expect("took exactly eight bytes");
        Ok(u64::from_be_bytes(raw))
    }

    /// The next fixed-width `N`-byte array.
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], FormatError> {
        Ok(self.take(N)?.try_into().expect("took exactly N bytes"))
    }

    /// The next `n` bytes as an owned `Vec`. The caller validates `n` against a fixed field
    /// bound before calling, so the copy is bounded; a truncated input still rejects.
    pub fn take_vec(&mut self, n: usize) -> Result<Vec<u8>, FormatError> {
        Ok(self.take(n)?.to_vec())
    }

    /// The exact magic bytes, rejecting anything else as [`FormatError::BadMagic`].
    pub fn magic(&mut self, expected: &[u8]) -> Result<(), FormatError> {
        if self.take(expected.len())? == expected {
            Ok(())
        } else {
            Err(FormatError::BadMagic)
        }
    }

    /// Reject any trailing bytes: a canonical artifact is consumed exactly.
    pub fn finish(self) -> Result<(), FormatError> {
        if self.pos == self.bytes.len() {
            Ok(())
        } else {
            Err(FormatError::TrailingBytes)
        }
    }
}

/// Append a big-endian `u32`.
pub(crate) fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

/// Append a big-endian `u64`.
pub(crate) fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_rejects_truncation_and_trailing_bytes() {
        let mut r = Reader::new(&[0x00, 0x00, 0x00, 0x05]);
        assert_eq!(r.u32(), Ok(5));
        assert_eq!(r.finish(), Ok(()));

        // One byte short of a u32.
        let mut r = Reader::new(&[0x00, 0x00, 0x00]);
        assert_eq!(r.u32(), Err(FormatError::Truncated));

        // A leftover byte after the last field.
        let mut r = Reader::new(&[0x01, 0xFF]);
        assert_eq!(r.u8(), Ok(0x01));
        assert_eq!(r.finish(), Err(FormatError::TrailingBytes));
    }

    #[test]
    fn magic_and_arrays_read_exactly() {
        let mut r = Reader::new(b"MW\x00\x01\x02\x03");
        assert_eq!(r.magic(b"MW"), Ok(()));
        assert_eq!(r.array::<4>(), Ok([0x00, 0x01, 0x02, 0x03]));
        assert_eq!(r.finish(), Ok(()));

        let mut r = Reader::new(b"XX");
        assert_eq!(r.magic(b"MW"), Err(FormatError::BadMagic));
    }

    #[test]
    fn distinct_malformations_carry_distinct_codes() {
        assert_eq!(
            FormatError::UnknownVersion { found: 9 }.code(),
            Code::StoreFormatVersion
        );
        assert_eq!(
            FormatError::LengthOverflow {
                field: FormatField::HeadMapEntries
            }
            .code(),
            Code::StoreLimit
        );
        assert_eq!(FormatError::DigestMismatch.code(), Code::StoreCorruption);
        assert_eq!(FormatError::BadMagic.code(), Code::StoreCorruption);
    }
}
