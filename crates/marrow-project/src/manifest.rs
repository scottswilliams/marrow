//! The project manifest, `marrow.toml`: a closed, versioned schema whose only
//! required key is an explicit language `edition`, plus the optional
//! `[dependencies]` table naming local source dependencies by relative path.
//!
//! The schema is deliberately tiny. Parsing is total over arbitrary text: any
//! input either yields a [`Manifest`] or a typed [`ManifestError`] carrying a
//! stable code, a typed reason, and — for a malformed-TOML fault the parser can
//! locate — a 1-based [`Position`]. Unknown keys reject rather than being
//! ignored, so a typo or a key from a future schema fails closed instead of
//! silently changing project meaning.

use marrow_codes::Code;
use std::fmt;

use crate::dependency::{
    Dependency, DependencyAlias, DependencyAliasReason, DependencyPath, DependencyPathReason,
};

/// The required manifest key: the language edition.
const EDITION_KEY: &str = "edition";

/// The optional manifest table: local source dependencies, alias to location.
const DEPENDENCIES_KEY: &str = "dependencies";

/// The single key one dependency entry admits.
const DEPENDENCY_PATH_KEY: &str = "path";

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
/// a `Manifest` value always names a supported edition and a validated set of
/// local dependencies, and nothing else.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Manifest {
    edition: Edition,
    /// Declared dependencies in alias order, at most one entry per alias.
    dependencies: Vec<Dependency>,
}

impl Manifest {
    /// The declared language edition.
    pub fn edition(&self) -> Edition {
        self.edition
    }

    /// The declared local dependencies, in alias order.
    pub fn dependencies(&self) -> &[Dependency] {
        &self.dependencies
    }

    /// Parse and validate the bytes of a `marrow.toml`.
    ///
    /// The schema is closed: the required `edition` key, whose value is one of the
    /// supported edition spellings, and the optional `[dependencies]` table, whose
    /// entries map a consumer-chosen alias to `{ path = "relative" }` and admit no
    /// other key. Malformed TOML, an unknown key, a missing edition, a non-string
    /// edition, an unsupported edition, or a misshapen dependency entry each reject
    /// with a typed [`ManifestError`].
    pub fn parse(source: &str) -> Result<Manifest, ManifestError> {
        let table: toml::Table =
            toml::from_str(source).map_err(|error| ManifestError::malformed(&error, source))?;

        // `toml::Table` iterates its keys in sorted order (no `preserve_order`
        // feature), so the first unknown key reported is deterministic regardless
        // of the order keys appear in the source.
        for key in table.keys() {
            if key != EDITION_KEY && key != DEPENDENCIES_KEY {
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

        let dependencies = match table.get(DEPENDENCIES_KEY) {
            None => Vec::new(),
            Some(value) => parse_dependencies(value)?,
        };

        Ok(Manifest {
            edition,
            dependencies,
        })
    }
}

/// Validate the `[dependencies]` table into alias-ordered entries. The table's own
/// shape is a closed-schema fault; an alias or path the shape admits but the
/// dependency vocabulary refuses carries its own `project.dependency_*` code.
fn parse_dependencies(value: &toml::Value) -> Result<Vec<Dependency>, ManifestError> {
    let table = value
        .as_table()
        .ok_or_else(ManifestError::dependencies_not_table)?;
    // `toml` rejects a duplicate key while parsing, so each alias appears once;
    // sorted iteration makes the first reported fault independent of source order.
    let mut dependencies = Vec::with_capacity(table.len());
    for (alias, entry) in table {
        let alias = DependencyAlias::parse(alias)
            .map_err(|reason| ManifestError::dependency_alias(alias, reason))?;
        let entry = entry
            .as_table()
            .ok_or_else(|| ManifestError::dependency_not_table(&alias))?;
        for key in entry.keys() {
            if key != DEPENDENCY_PATH_KEY {
                return Err(ManifestError::unknown_dependency_key(&alias, key));
            }
        }
        let spelling = entry
            .get(DEPENDENCY_PATH_KEY)
            .ok_or_else(|| ManifestError::dependency_missing_path(&alias))?
            .as_str()
            .ok_or_else(|| ManifestError::dependency_path_not_string(&alias))?;
        let path = DependencyPath::parse(spelling)
            .map_err(|reason| ManifestError::dependency_path(&alias, spelling, reason))?;
        dependencies.push(Dependency::new(alias, path));
    }
    Ok(dependencies)
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
    /// The `[dependencies]` table, or one of its entries, has the wrong shape:
    /// it is not a table, an entry declares a key other than `path`, or an
    /// entry's `path` is missing or not a string.
    DependencyShape {
        /// The alias whose entry is misshapen, or `None` for the table itself.
        alias: Option<DependencyAlias>,
        /// What the shape fault was.
        fault: DependencyShapeFault,
    },
    /// A declared alias cannot name a dependency.
    DependencyAlias {
        /// The rejected alias spelling.
        alias: String,
        /// Why it was rejected.
        reason: DependencyAliasReason,
    },
    /// A declared path cannot name a dependency's location.
    DependencyPath {
        /// The alias whose path was rejected.
        alias: DependencyAlias,
        /// The rejected path spelling.
        path: String,
        /// Why it was rejected.
        reason: DependencyPathReason,
    },
}

/// A closed-schema fault in the shape of the `[dependencies]` table.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DependencyShapeFault {
    /// `dependencies` is present but is not a table.
    NotATable,
    /// An entry is present but is not a table.
    EntryNotATable,
    /// An entry declares a key outside the closed entry schema.
    UnknownKey {
        /// The offending key.
        key: String,
    },
    /// An entry declares no `path`.
    MissingPath,
    /// An entry's `path` is present but not a string.
    PathNotString,
}

/// An invalid project manifest. Carries a stable [`Code`] — `config.invalid` for
/// a schema fault, `project.dependency_alias` or `project.dependency_path` for a
/// declared dependency the schema admits but the dependency vocabulary refuses —
/// a typed [`ManifestErrorKind`], a human message, and, for a malformed-TOML fault
/// the parser locates, a 1-based [`Position`].
///
/// Fields are private and every constructor is owner-private, so a `ManifestError`
/// is always the exact typed code/kind/message/position `parse` produced; an
/// inconsistent combination is unrepresentable outside this owner. The read-only
/// accessors expose the typed [`Code`] rather than a spelling.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ManifestError {
    code: Code,
    kind: ManifestErrorKind,
    message: String,
    /// The 1-based line and column of a malformed-TOML fault. A validation fault
    /// with no single source point leaves it `None`.
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
        Self::coded(Code::ConfigInvalid, kind, message)
    }

    fn coded(code: Code, kind: ManifestErrorKind, message: impl Into<String>) -> Self {
        Self {
            code,
            kind,
            message: message.into(),
            position: None,
        }
    }

    fn shape(alias: Option<&DependencyAlias>, fault: DependencyShapeFault) -> Self {
        let subject = match alias {
            Some(alias) => format!("dependency `{}`", alias.as_str()),
            None => "`dependencies`".to_string(),
        };
        let explanation = match &fault {
            DependencyShapeFault::NotATable => {
                "must be a table of aliases, for example `[dependencies]`".to_string()
            }
            DependencyShapeFault::EntryNotATable => {
                "must be a table, for example `{ path = \"../lib\" }`".to_string()
            }
            DependencyShapeFault::UnknownKey { key } => format!(
                "declares unknown key `{key}`; the only supported key is `{DEPENDENCY_PATH_KEY}`"
            ),
            DependencyShapeFault::MissingPath => {
                "must declare a `path`, for example `{ path = \"../lib\" }`".to_string()
            }
            DependencyShapeFault::PathNotString => {
                "must declare `path` as a string, for example `{ path = \"../lib\" }`".to_string()
            }
        };
        Self::new(
            ManifestErrorKind::DependencyShape {
                alias: alias.cloned(),
                fault,
            },
            format!("{subject} {explanation}"),
        )
    }

    fn dependencies_not_table() -> Self {
        Self::shape(None, DependencyShapeFault::NotATable)
    }

    fn dependency_not_table(alias: &DependencyAlias) -> Self {
        Self::shape(Some(alias), DependencyShapeFault::EntryNotATable)
    }

    fn unknown_dependency_key(alias: &DependencyAlias, key: &str) -> Self {
        Self::shape(
            Some(alias),
            DependencyShapeFault::UnknownKey {
                key: key.to_string(),
            },
        )
    }

    fn dependency_missing_path(alias: &DependencyAlias) -> Self {
        Self::shape(Some(alias), DependencyShapeFault::MissingPath)
    }

    fn dependency_path_not_string(alias: &DependencyAlias) -> Self {
        Self::shape(Some(alias), DependencyShapeFault::PathNotString)
    }

    fn dependency_alias(alias: &str, reason: DependencyAliasReason) -> Self {
        let explanation = match &reason {
            DependencyAliasReason::TooLong { limit, actual } => {
                format!("is {actual} bytes, over the {limit}-byte dependency-alias limit")
            }
            DependencyAliasReason::Placeholder => {
                "is the placeholder `_`, which names nothing".to_string()
            }
            _ => "must be an identifier: a letter or `_` followed by letters, digits, or `_`"
                .to_string(),
        };
        Self::coded(
            Code::ProjectDependencyAlias,
            ManifestErrorKind::DependencyAlias {
                alias: alias.to_string(),
                reason,
            },
            format!("dependency alias `{alias}` {explanation}"),
        )
    }

    fn dependency_path(alias: &DependencyAlias, path: &str, reason: DependencyPathReason) -> Self {
        let explanation = match reason {
            DependencyPathReason::Absolute => {
                "must be relative to the project root, not absolute".to_string()
            }
            DependencyPathReason::NonCanonical => {
                "must be a canonical forward-slash relative path with no control character, no \
                 empty or `.` segment, and no `..` segment after a named one"
                    .to_string()
            }
            DependencyPathReason::TooLong { limit, actual } => {
                format!("is {actual} bytes, over the {limit}-byte dependency-path limit")
            }
        };
        Self::coded(
            Code::ProjectDependencyPath,
            ManifestErrorKind::DependencyPath {
                alias: alias.clone(),
                path: path.to_string(),
                reason,
            },
            format!(
                "dependency `{}` path `{path}` {explanation}",
                alias.as_str()
            ),
        )
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
            format!(
                "unknown manifest key `{key}`; the supported keys are `edition` and `dependencies`"
            ),
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
