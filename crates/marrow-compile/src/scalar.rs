//! The language scalar vocabulary.
//!
//! This is the compiler's owner of the scalar *language* classification, refounded
//! here out of the storage engine (design §F). It is distinct from the kernel's
//! runtime representation (`RuntimeScalar`/`KeyScalar`); the image type tags are
//! the frozen bridge between them.

use marrow_image::Scalar;

/// A scalar language type in the compiled subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScalarType {
    Int,
    Bool,
    Text,
    Bytes,
    /// A proleptic-Gregorian calendar day. The temporal scalar domain (calendar,
    /// canonical text codec, arithmetic) is owned by `marrow-temporal`.
    Date,
    /// A UTC nanosecond instant.
    Instant,
    /// A signed nanosecond span.
    Duration,
}

impl ScalarType {
    /// The scalar named by a source type spelling, or `None` for anything else.
    pub fn from_spelling(text: &str) -> Option<Self> {
        match text {
            "int" => Some(ScalarType::Int),
            "bool" => Some(ScalarType::Bool),
            "string" => Some(ScalarType::Text),
            "bytes" => Some(ScalarType::Bytes),
            "date" => Some(ScalarType::Date),
            "instant" => Some(ScalarType::Instant),
            "duration" => Some(ScalarType::Duration),
            _ => None,
        }
    }

    /// The canonical language spelling.
    pub fn spelling(self) -> &'static str {
        match self {
            ScalarType::Int => "int",
            ScalarType::Bool => "bool",
            ScalarType::Text => "string",
            ScalarType::Bytes => "bytes",
            ScalarType::Date => "date",
            ScalarType::Instant => "instant",
            ScalarType::Duration => "duration",
        }
    }

    /// Whether this scalar is a temporal type (`date`/`instant`/`duration`).
    pub fn is_temporal(self) -> bool {
        matches!(
            self,
            ScalarType::Date | ScalarType::Instant | ScalarType::Duration
        )
    }

    /// The image type tag this scalar lowers to.
    pub fn image(self) -> Scalar {
        match self {
            ScalarType::Int => Scalar::Int,
            ScalarType::Bool => Scalar::Bool,
            ScalarType::Text => Scalar::Text,
            ScalarType::Bytes => Scalar::Bytes,
            ScalarType::Date => Scalar::Date,
            ScalarType::Instant => Scalar::Instant,
            ScalarType::Duration => Scalar::Duration,
        }
    }
}
