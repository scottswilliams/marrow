use std::fmt;

use marrow_store::StoreError;

use crate::ScalarType;

/// A typed reason a data query, walk, or child page could not be resolved.
///
/// Each variant carries the structured facts a caller needs to act on the
/// failure (which root, which member, the expected and found key types) rather
/// than a pre-rendered sentence. The boundary that surfaces the error renders
/// it through [`fmt::Display`]; checker, serve, and CLI logic match on the
/// variant, never on the rendered text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    /// The path did not begin with a saved root segment.
    MissingRoot,
    /// No saved root named `^{root}` is declared.
    UnknownRoot { root: String },
    /// More identity keys were supplied than `^{root}` declares.
    TooManyIdentityKeys { root: String },
    /// An identity key has the wrong scalar type for `^{root}`.
    IdentityKeyType {
        root: String,
        expected: ScalarType,
        found: ScalarType,
    },
    /// Member access was reached before `^{root}`'s identity keys were complete.
    MissingIdentityKeys { root: String, expected: usize },
    /// A key segment followed something that takes no key.
    UnexpectedKey,
    /// No member of the named flavor exists at this point in the path.
    UnknownMember { flavor: MemberFlavor, name: String },
    /// More keys were supplied than the keyed member declares.
    TooManyMemberKeys { member: String },
    /// A member key has the wrong scalar type for its declaration.
    MemberKeyType {
        member: String,
        expected: ScalarType,
        found: ScalarType,
    },
    /// A keyed member needs all its keys before any nested access.
    IncompleteMemberKeys { member: String },
    /// A page or walk was asked for with a zero limit.
    ZeroLimit,
    /// A resume cursor resolved outside the requested path.
    CursorOutsidePath,
    /// A resume cursor did not name a walk position (a value path).
    CursorNotAPosition,
    /// A resume cursor named no walk entry under the requested path.
    CursorNotAnEntry,
    /// A declared-member listing is not a paged scan and takes no cursor.
    MembersTakeNoCursor,
    /// The path names a leaf or record with no scannable children.
    NoChildScan,
    /// A checked catalog id was missing or malformed in the program facts.
    CorruptCatalogId(StoreError),
}

/// Which member-naming flavor a query segment used, so an unknown-member error
/// can name the same flavor the caller wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberFlavor {
    Field,
    Layer,
    Member,
}

impl MemberFlavor {
    fn noun(self) -> &'static str {
        match self {
            Self::Field => "field",
            Self::Layer => "layer",
            Self::Member => "member",
        }
    }
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRoot => {
                write!(f, "path must start with a saved root, such as `^books`")
            }
            Self::UnknownRoot { root } => write!(f, "unknown saved root `^{root}`"),
            Self::TooManyIdentityKeys { root } => {
                write!(f, "`^{root}` has too many identity keys")
            }
            Self::IdentityKeyType {
                root,
                expected,
                found,
            } => write!(
                f,
                "identity key is a {} where `^{root}` declares {}",
                found.name(),
                expected.name()
            ),
            Self::MissingIdentityKeys { root, expected } => write!(
                f,
                "`^{root}` expects {expected} identity key(s) before member access"
            ),
            Self::UnexpectedKey => {
                write!(f, "a key must follow a saved root or a keyed member")
            }
            Self::UnknownMember { flavor, name } => {
                write!(f, "unknown saved {} `{name}`", flavor.noun())
            }
            Self::TooManyMemberKeys { member } => {
                write!(f, "member `{member}` has too many keys")
            }
            Self::MemberKeyType {
                member,
                expected,
                found,
            } => write!(
                f,
                "`{member}` key is a {} where the schema declares {}",
                found.name(),
                expected.name()
            ),
            Self::IncompleteMemberKeys { member } => {
                write!(f, "member `{member}` needs all keys before nested access")
            }
            Self::ZeroLimit => write!(f, "`limit` must be greater than zero"),
            Self::CursorOutsidePath => write!(f, "`cursor` is outside the requested path"),
            Self::CursorNotAPosition => write!(f, "`cursor` is not a data walk position"),
            Self::CursorNotAnEntry => write!(f, "`cursor` does not name a data walk entry"),
            Self::MembersTakeNoCursor => write!(
                f,
                "declared members are not a paged child scan, so they take no `cursor`"
            ),
            Self::NoChildScan => {
                write!(f, "the path names a leaf with no scannable children")
            }
            Self::CorruptCatalogId(error) => write!(f, "{error}"),
        }
    }
}

impl From<StoreError> for QueryError {
    fn from(error: StoreError) -> Self {
        Self::CorruptCatalogId(error)
    }
}
