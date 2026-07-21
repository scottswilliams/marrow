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

/// Why a persisted store artifact failed to decode. Callers match the variant; the stable
/// dotted [`Code`] is for rendering only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// The artifact does not begin with its expected magic — it is not this kind of store
    /// artifact at all.
    BadMagic,
    /// The artifact records a container version this build does not read. A future version
    /// is a typed refusal, never a best-effort decode (FR01 §6).
    UnknownVersion { found: u8 },
    /// The bytes end before a field the grammar requires — a truncated or torn artifact.
    Truncated,
    /// Bytes remain after the artifact's last field — a canonical artifact is consumed
    /// exactly, so trailing bytes are a malformed artifact, never ignored.
    TrailingBytes,
    /// A length or count field exceeds the fixed bound the grammar allows before any
    /// allocation, so a hostile length can never drive an unbounded reservation.
    LengthOverflow { field: &'static str },
    /// A discriminant or flag byte is outside the closed set the grammar defines.
    UnknownDiscriminant { field: &'static str },
    /// The recomputed digest does not match the sealed digest — the artifact's body was
    /// altered or is inconsistent with its seal.
    DigestMismatch,
    /// A structural invariant of the decoded artifact is violated (for example a head map
    /// that reuses a number or a ledger id), so the bytes are not a coherent artifact.
    Malformed { reason: &'static str },
}

impl FormatError {
    /// The stable dotted code a tool reports. A version this build does not read is a
    /// format-version refusal; a length beyond its bound is a representational limit;
    /// every other malformation is store corruption (the persisted bytes do not decode).
    pub fn code(&self) -> &'static str {
        match self {
            FormatError::UnknownVersion { .. } => Code::StoreFormatVersion.as_str(),
            FormatError::LengthOverflow { .. } => Code::StoreLimit.as_str(),
            FormatError::BadMagic
            | FormatError::Truncated
            | FormatError::TrailingBytes
            | FormatError::UnknownDiscriminant { .. }
            | FormatError::DigestMismatch
            | FormatError::Malformed { .. } => Code::StoreCorruption.as_str(),
        }
    }
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormatError::BadMagic => write!(f, "not a Marrow store artifact (bad magic)"),
            FormatError::UnknownVersion { found } => {
                write!(f, "unsupported store artifact version {found}")
            }
            FormatError::Truncated => write!(f, "the store artifact is truncated"),
            FormatError::TrailingBytes => write!(f, "the store artifact has trailing bytes"),
            FormatError::LengthOverflow { field } => {
                write!(f, "the store artifact field {field} exceeds its bound")
            }
            FormatError::UnknownDiscriminant { field } => {
                write!(
                    f,
                    "the store artifact field {field} has an unknown discriminant"
                )
            }
            FormatError::DigestMismatch => write!(f, "the store artifact digest does not match"),
            FormatError::Malformed { reason } => {
                write!(f, "the store artifact is malformed: {reason}")
            }
        }
    }
}

impl std::error::Error for FormatError {}

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
            "store.format_version"
        );
        assert_eq!(
            FormatError::LengthOverflow { field: "map" }.code(),
            "store.limit"
        );
        assert_eq!(FormatError::DigestMismatch.code(), "store.corruption");
        assert_eq!(FormatError::BadMagic.code(), "store.corruption");
    }
}
