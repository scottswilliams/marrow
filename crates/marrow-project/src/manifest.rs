//! The project manifest, `marrow.toml`: a closed, versioned schema whose only
//! required key is an explicit language `edition`.
//!
//! The schema is deliberately tiny. Parsing is total over arbitrary text: any
//! input either yields a [`Manifest`] or a typed [`ManifestError`] carrying a
//! stable code, a typed reason, and — for a malformed-TOML fault the parser can
//! locate — a 1-based [`Position`]. Unknown keys reject rather than being
//! ignored, so a typo or a key from a future schema fails closed instead of
//! silently changing project meaning.

use marrow_codes::Code;
use std::fmt;

/// The single manifest key the closed schema admits.
const EDITION_KEY: &str = "edition";

/// The language edition a manifest declares. The set is closed; an unrecognized
/// spelling rejects rather than inheriting a moving toolchain default.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edition {
    /// The `2026` edition, the only edition this build supports.
    E2026,
}

impl Edition {
    /// The edition a fresh `marrow init` project declares.
    pub const CURRENT: Edition = Edition::E2026;

    /// The canonical manifest spelling of this edition.
    pub const fn as_str(self) -> &'static str {
        match self {
            Edition::E2026 => "2026",
        }
    }

    fn parse(spelling: &str) -> Option<Edition> {
        match spelling {
            "2026" => Some(Edition::E2026),
            _ => None,
        }
    }
}

/// A validated project manifest. Constructed only through [`Manifest::parse`], so
/// a `Manifest` value always names a supported edition and nothing else.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Manifest {
    edition: Edition,
}

impl Manifest {
    /// The declared language edition.
    pub fn edition(&self) -> Edition {
        self.edition
    }

    /// Parse and validate the bytes of a `marrow.toml`.
    ///
    /// The schema is closed: exactly one key, `edition`, whose value is one of
    /// the supported edition spellings. Malformed TOML, an unknown key, a missing
    /// edition, a non-string edition, or an unsupported edition each reject with a
    /// typed [`ManifestError`].
    pub fn parse(source: &str) -> Result<Manifest, ManifestError> {
        let table: toml::Table =
            toml::from_str(source).map_err(|error| ManifestError::malformed(&error, source))?;

        // `toml::Table` iterates its keys in sorted order (no `preserve_order`
        // feature), so the first unknown key reported is deterministic regardless
        // of the order keys appear in the source.
        for key in table.keys() {
            if key != EDITION_KEY {
                return Err(ManifestError::unknown_key(key));
            }
        }

        let value = table
            .get(EDITION_KEY)
            .ok_or_else(ManifestError::missing_edition)?;
        let spelling = value
            .as_str()
            .ok_or_else(ManifestError::edition_not_string)?;
        let edition =
            Edition::parse(spelling).ok_or_else(|| ManifestError::unsupported_edition(spelling))?;

        Ok(Manifest { edition })
    }
}

/// A 1-based position inside a `marrow.toml`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Position {
    pub line: u32,
    pub column: u32,
}

/// The typed reason a `marrow.toml` failed to parse or validate.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ManifestErrorKind {
    /// The bytes are not well-formed TOML.
    Malformed,
    /// The manifest declares a key outside the closed schema.
    UnknownKey { key: String },
    /// The manifest declares no `edition`.
    MissingEdition,
    /// The `edition` value is present but not a string.
    EditionNotString,
    /// The `edition` value is a string this build does not support.
    UnsupportedEdition { edition: String },
}

/// An invalid project manifest. Carries the stable [`Code::ConfigInvalid`] code,
/// a typed [`ManifestErrorKind`], a human message, and — for a malformed-TOML
/// fault the parser locates — a 1-based [`Position`].
///
/// Fields are private and every constructor is owner-private, so a `ManifestError`
/// is always the exact typed code/kind/message/position triple `parse` produced;
/// a hostile or inconsistent combination is unrepresentable outside this owner.
/// The read-only accessors expose the typed [`Code`] rather than a spelling.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ManifestError {
    code: Code,
    kind: ManifestErrorKind,
    message: String,
    /// The 1-based line and column of a malformed-TOML fault. A validation fault
    /// with no single source point leaves it `None`, keeping the located position
    /// a machine fact in the span rather than only prose a client must parse.
    position: Option<Position>,
}

impl ManifestError {
    /// The stable diagnostic code this fault carries.
    pub fn code(&self) -> Code {
        self.code
    }

    /// The typed reason the manifest was rejected.
    pub fn kind(&self) -> &ManifestErrorKind {
        &self.kind
    }

    /// The human-readable message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The 1-based position of a located malformed-TOML fault, or `None` when the
    /// fault has no single source point. Only [`ManifestErrorKind::Malformed`]
    /// ever carries a position.
    pub fn position(&self) -> Option<Position> {
        self.position
    }

    fn new(kind: ManifestErrorKind, message: impl Into<String>) -> Self {
        Self {
            code: Code::ConfigInvalid,
            kind,
            message: message.into(),
            position: None,
        }
    }

    fn malformed(error: &toml::de::Error, source: &str) -> Self {
        let position = error.span().map(|span| line_column(source, span.start));
        Self {
            code: Code::ConfigInvalid,
            kind: ManifestErrorKind::Malformed,
            message: error.message().to_string(),
            position,
        }
    }

    fn unknown_key(key: &str) -> Self {
        Self::new(
            ManifestErrorKind::UnknownKey {
                key: key.to_string(),
            },
            format!("unknown manifest key `{key}`; the only supported key is `edition`"),
        )
    }

    fn missing_edition() -> Self {
        Self::new(
            ManifestErrorKind::MissingEdition,
            "manifest must declare an `edition`, for example `edition = \"2026\"`",
        )
    }

    fn edition_not_string() -> Self {
        Self::new(
            ManifestErrorKind::EditionNotString,
            "`edition` must be a string, for example `edition = \"2026\"`",
        )
    }

    fn unsupported_edition(edition: &str) -> Self {
        Self::new(
            ManifestErrorKind::UnsupportedEdition {
                edition: edition.to_string(),
            },
            format!("unsupported edition `{edition}`; this build supports `2026`"),
        )
    }
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for ManifestError {}

/// Convert a byte offset into 1-based line and column. A column counts UTF-8
/// characters from the line start, so a multibyte character advances the column
/// by one, matching how an editor renders the position.
fn line_column(source: &str, offset: usize) -> Position {
    let mut line = 1u32;
    let mut column = 1u32;
    for (index, character) in source.char_indices() {
        if index >= offset {
            break;
        }
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    Position { line, column }
}
