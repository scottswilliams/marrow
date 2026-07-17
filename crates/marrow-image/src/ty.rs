//! The image-level type reference (design §C). It is named `ImageType` rather than
//! the design's Rust spelling because that spelling is a forbidden identifier the
//! syntax crate reserves for its deleted string-carrier — this structural tag,
//! carrying no source text, is a different concept.
//!
//! One `u8` tag names the base type, with the high bit `0x80` marking an optional
//! wrapper. This is the single spelling of a type everywhere the image records one
//! (record field, param, return, `VacantLoad` operand); position restrictions are
//! enforced by the encoder that writes each and rechecked by the verifier.

/// A bare image scalar. The runtime representation vocabulary (`RuntimeScalar`,
/// `KeyScalar`) lives in the kernel and bridges to these tags; the temporal scalar
/// domain (calendar, canonical text codec, arithmetic) lives in `marrow-temporal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scalar {
    Int,
    Bool,
    Text,
    Bytes,
    Date,
    Instant,
    Duration,
}

/// Base-type tag bytes (low seven bits of a `ImageType` byte).
pub const TAG_UNIT: u8 = 0x00;
pub const TAG_INT: u8 = 0x01;
pub const TAG_BOOL: u8 = 0x02;
pub const TAG_TEXT: u8 = 0x03;
pub const TAG_RECORD: u8 = 0x04;
pub const TAG_BYTES: u8 = 0x05;
pub const TAG_ENUM: u8 = 0x06;
pub const TAG_COLLECTION: u8 = 0x07;
pub const TAG_DATE: u8 = 0x08;
pub const TAG_INSTANT: u8 = 0x09;
pub const TAG_DURATION: u8 = 0x0A;

/// The optional-wrapper flag bit.
pub const OPTIONAL_FLAG: u8 = 0x80;

impl Scalar {
    /// The bare base tag for this scalar.
    pub const fn tag(self) -> u8 {
        match self {
            Scalar::Int => TAG_INT,
            Scalar::Bool => TAG_BOOL,
            Scalar::Text => TAG_TEXT,
            Scalar::Bytes => TAG_BYTES,
            Scalar::Date => TAG_DATE,
            Scalar::Instant => TAG_INSTANT,
            Scalar::Duration => TAG_DURATION,
        }
    }
}

/// A type as recorded in the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageType {
    Unit,
    Scalar {
        scalar: Scalar,
        optional: bool,
    },
    Record {
        idx: u16,
        optional: bool,
    },
    /// A closed enum value, by ENUMS-table index. Mirrors `Record`: a one-byte
    /// tag plus a big-endian `u16` index.
    Enum {
        idx: u16,
        optional: bool,
    },
    /// A finite collection value (a `List<T>` or ordered `Map<K, V>`), by
    /// COLLTYPES-table index. Mirrors `Record`/`Enum`: a one-byte tag plus a
    /// big-endian `u16` index. The element/key/value types are recorded once in
    /// the COLLTYPES entry, so a nested collection reaches its inner type through
    /// that table rather than inlining it here (keeping `ImageType` `Copy`).
    Collection {
        idx: u16,
        optional: bool,
    },
}

impl ImageType {
    pub const fn scalar(scalar: Scalar) -> Self {
        ImageType::Scalar {
            scalar,
            optional: false,
        }
    }

    pub const fn opt_scalar(scalar: Scalar) -> Self {
        ImageType::Scalar {
            scalar,
            optional: true,
        }
    }

    pub const fn is_optional(self) -> bool {
        match self {
            ImageType::Unit => false,
            ImageType::Scalar { optional, .. }
            | ImageType::Record { optional, .. }
            | ImageType::Enum { optional, .. }
            | ImageType::Collection { optional, .. } => optional,
        }
    }

    /// The number of bytes [`ImageType::encode`] appends: one tag byte, plus a
    /// big-endian `u16` index for a record, enum, or collection base.
    pub(crate) fn encoded_len(self) -> usize {
        match self {
            ImageType::Unit | ImageType::Scalar { .. } => 1,
            ImageType::Record { .. } | ImageType::Enum { .. } | ImageType::Collection { .. } => 3,
        }
    }

    /// Append the canonical `ImageType` bytes: one tag byte, plus a big-endian `u16`
    /// record index when the base is a record.
    pub(crate) fn encode(self, out: &mut Vec<u8>) {
        match self {
            ImageType::Unit => out.push(TAG_UNIT),
            ImageType::Scalar { scalar, optional } => {
                out.push(scalar.tag() | if optional { OPTIONAL_FLAG } else { 0 });
            }
            ImageType::Record { idx, optional } => {
                out.push(TAG_RECORD | if optional { OPTIONAL_FLAG } else { 0 });
                out.extend_from_slice(&idx.to_be_bytes());
            }
            ImageType::Enum { idx, optional } => {
                out.push(TAG_ENUM | if optional { OPTIONAL_FLAG } else { 0 });
                out.extend_from_slice(&idx.to_be_bytes());
            }
            ImageType::Collection { idx, optional } => {
                out.push(TAG_COLLECTION | if optional { OPTIONAL_FLAG } else { 0 });
                out.extend_from_slice(&idx.to_be_bytes());
            }
        }
    }
}
