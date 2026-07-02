use std::fmt;

use crate::ScalarType;

/// A typed reason a data path, walk, or child page is malformed.
///
/// Every variant is a client-facing request error: the path or page arguments
/// did not describe a valid path. Server-side faults such as a missing or
/// malformed checked catalog id are not path malformity; they stay
/// [`crate::tooling::ToolingError::Store`] so they keep the store code at the
/// boundary. Each variant carries the structured facts a caller needs (which
/// root, which member, the expected and found key types) rather than a
/// pre-rendered sentence. The boundary that surfaces the error renders it
/// through [`fmt::Display`]; callers match on the variant, never on the
/// rendered text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataPathError {
    /// The path did not begin with a saved root segment.
    MissingRoot,
    /// No saved root named `^{root}` is declared.
    UnknownRoot { root: String },
    /// No accepted saved root carries this store catalog id in the checked saved tree.
    UnknownRootCatalogId { store_catalog_id: String },
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
    /// No accepted member of the requested flavor carries this catalog id at this point in the path.
    UnknownMemberCatalogId {
        flavor: MemberFlavor,
        member_catalog_id: String,
    },
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
}

/// Which member-naming flavor a path segment used, so an unknown-member error
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

/// The stable code for a path that parses but the checked schema cannot resolve to a
/// declared address: an undeclared root or member, or an identity or member key with a
/// scalar type or arity the schema does not declare. Such a path is well-formed input
/// the schema cannot resolve, so it is reported as a typed `data` diagnostic rather
/// than a command-line usage error.
pub const UNKNOWN_PATH_CODE: &str = "data.unknown_path";

impl DataPathError {
    /// The typed diagnostic code for a schema-resolution failure: a well-formed path
    /// the checked schema cannot resolve to a declared address, whether because it
    /// names an undeclared root or member or because an identity or member key has the
    /// wrong scalar type or arity for what the schema declares. Every such variant
    /// consults the schema, so it is a typed resolution failure (exit `1`, JSON
    /// envelope), not a command-line usage error. The remaining variants describe a
    /// path that is malformed or misused independent of any schema — a missing root,
    /// a stray key, a bad page cursor or limit — which the CLI boundary reports as a
    /// usage error, so they carry no resolution code. The match is exhaustive with no
    /// catch-all: a new schema-dependent variant cannot silently default to the usage
    /// channel.
    pub fn resolution_code(&self) -> Option<&'static str> {
        match self {
            Self::UnknownRoot { .. }
            | Self::UnknownRootCatalogId { .. }
            | Self::UnknownMember { .. }
            | Self::UnknownMemberCatalogId { .. }
            | Self::TooManyIdentityKeys { .. }
            | Self::IdentityKeyType { .. }
            | Self::MissingIdentityKeys { .. }
            | Self::TooManyMemberKeys { .. }
            | Self::MemberKeyType { .. }
            | Self::IncompleteMemberKeys { .. } => Some(UNKNOWN_PATH_CODE),
            Self::MissingRoot
            | Self::UnexpectedKey
            | Self::ZeroLimit
            | Self::CursorOutsidePath
            | Self::CursorNotAPosition
            | Self::CursorNotAnEntry
            | Self::MembersTakeNoCursor
            | Self::NoChildScan => None,
        }
    }
}

impl fmt::Display for DataPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRoot => {
                write!(f, "path must start with a saved root, such as `^books`")
            }
            Self::UnknownRoot { root } => write!(f, "unknown saved root `^{root}`"),
            Self::UnknownRootCatalogId { store_catalog_id } => {
                write!(f, "unknown saved root catalog id `{store_catalog_id}`")
            }
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
            Self::UnknownMemberCatalogId {
                flavor,
                member_catalog_id,
            } => write!(
                f,
                "unknown saved {} catalog id `{member_catalog_id}`",
                flavor.noun()
            ),
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScalarType;

    /// Every schema-dependent resolution failure — including a wrong-typed or
    /// wrong-arity identity or member key — reports a typed resolution code so the
    /// boundary routes it to the exit-1 JSON envelope, never the exit-2 usage channel.
    #[test]
    fn schema_dependent_variants_carry_a_resolution_code() {
        let schema_dependent = [
            DataPathError::UnknownRoot {
                root: "books".into(),
            },
            DataPathError::UnknownRootCatalogId {
                store_catalog_id: "x".into(),
            },
            DataPathError::UnknownMember {
                flavor: MemberFlavor::Field,
                name: "f".into(),
            },
            DataPathError::UnknownMemberCatalogId {
                flavor: MemberFlavor::Field,
                member_catalog_id: "x".into(),
            },
            DataPathError::TooManyIdentityKeys {
                root: "books".into(),
            },
            DataPathError::IdentityKeyType {
                root: "books".into(),
                expected: ScalarType::Int,
                found: ScalarType::Str,
            },
            DataPathError::MissingIdentityKeys {
                root: "books".into(),
                expected: 1,
            },
            DataPathError::TooManyMemberKeys {
                member: "scores".into(),
            },
            DataPathError::MemberKeyType {
                member: "scores".into(),
                expected: ScalarType::Int,
                found: ScalarType::Str,
            },
            DataPathError::IncompleteMemberKeys {
                member: "scores".into(),
            },
        ];
        for error in schema_dependent {
            assert_eq!(
                error.resolution_code(),
                Some(UNKNOWN_PATH_CODE),
                "{error:?} consults the schema and must be a resolution failure",
            );
        }
    }

    /// A path malformed or misused independent of any schema stays a usage error.
    #[test]
    fn schema_independent_variants_carry_no_resolution_code() {
        let usage = [
            DataPathError::MissingRoot,
            DataPathError::UnexpectedKey,
            DataPathError::ZeroLimit,
            DataPathError::CursorOutsidePath,
            DataPathError::CursorNotAPosition,
            DataPathError::CursorNotAnEntry,
            DataPathError::MembersTakeNoCursor,
            DataPathError::NoChildScan,
        ];
        for error in usage {
            assert_eq!(error.resolution_code(), None, "{error:?} is a usage error");
        }
    }
}
