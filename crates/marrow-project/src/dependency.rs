//! The `[dependencies]` vocabulary: the consumer-chosen alias and the relative
//! path a project declares for one local source dependency.
//!
//! An alias is a single identifier the *consuming* project chooses; the
//! dependency's directory name and its own manifest supply no name. A path is
//! relative to the consuming root, so a project and its dependencies relocate
//! together. Both are validated at construction, so a [`DependencyAlias`] is
//! always a legal module segment and a [`DependencyPath`] is always a canonical
//! relative spelling; whether that spelling names a real project on disk is the
//! physical adapter's question, not this owner's.

use crate::identity::{ModuleName, is_placeholder};

/// The inclusive maximum UTF-8 byte length of a [`DependencyAlias`]. The alias
/// becomes the first segment of every module the dependency contributes, so it is
/// bounded well below the module-name maximum.
pub const MAX_DEPENDENCY_ALIAS_BYTES: usize = 64;

/// The inclusive maximum UTF-8 byte length of a [`DependencyPath`], matching the
/// source-path maximum.
pub const MAX_DEPENDENCY_PATH_BYTES: usize = crate::identity::MAX_FILE_IDENTITY_BYTES;

/// The consumer-chosen name of one declared dependency: a single identifier that
/// roots every module the dependency contributes. Constructed only through
/// [`DependencyAlias::parse`].
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DependencyAlias(String);

impl DependencyAlias {
    /// Validate a manifest alias spelling: a letter or `_` followed by letters,
    /// digits, or `_`, within [`MAX_DEPENDENCY_ALIAS_BYTES`], and never the placeholder
    /// `_` alone.
    pub fn parse(spelling: &str) -> Result<DependencyAlias, DependencyAliasReason> {
        let mut characters = spelling.chars();
        let start_is_legal = characters
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_');
        if !start_is_legal || !characters.all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(DependencyAliasReason::NotIdentifier);
        }
        if is_placeholder(spelling) {
            return Err(DependencyAliasReason::Placeholder);
        }
        if spelling.len() > MAX_DEPENDENCY_ALIAS_BYTES {
            return Err(DependencyAliasReason::TooLong {
                limit: MAX_DEPENDENCY_ALIAS_BYTES,
                actual: spelling.len(),
            });
        }
        Ok(DependencyAlias(spelling.to_string()))
    }

    /// The alias spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a declared alias cannot name a dependency.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DependencyAliasReason {
    /// The spelling is not an identifier.
    NotIdentifier,
    /// The spelling is the placeholder `_`, which names nothing.
    Placeholder,
    /// The spelling is an identifier but longer than
    /// [`MAX_DEPENDENCY_ALIAS_BYTES`] UTF-8 bytes.
    TooLong {
        /// The inclusive maximum alias length, in UTF-8 bytes.
        limit: usize,
        /// The offending alias's length, in UTF-8 bytes.
        actual: usize,
    },
    /// The alias is the first segment of a module the root project declares, so
    /// the alias-rooted path would be ambiguous.
    RootModuleCollision {
        /// The root module whose first segment the alias occupies.
        module: ModuleName,
    },
    /// Two captured origins claim the alias.
    Duplicate,
    /// A captured origin claims an alias the manifest does not declare.
    Undeclared,
    /// The manifest declares the alias but no origin was captured for it.
    Uncaptured,
}

/// The relative location of one declared dependency, as the manifest spells it.
/// Canonical, relative, and forward-slash separated; `..` may appear only in the
/// leading run, so a path steps out of the consuming root and then only descends.
/// Constructed only through [`DependencyPath::parse`].
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DependencyPath(String);

impl DependencyPath {
    /// Validate a manifest path spelling.
    pub fn parse(spelling: &str) -> Result<DependencyPath, DependencyPathReason> {
        if spelling.is_empty()
            || spelling.contains('\\')
            || spelling.chars().any(|c| c.is_ascii_control())
        {
            return Err(DependencyPathReason::NonCanonical);
        }
        if spelling.starts_with('/') {
            return Err(DependencyPathReason::Absolute);
        }
        let mut descended = false;
        for segment in spelling.split('/') {
            match segment {
                "" | "." => return Err(DependencyPathReason::NonCanonical),
                // A `..` after a descending segment would re-enter the tree from
                // above and give one directory two spellings.
                ".." if descended => return Err(DependencyPathReason::NonCanonical),
                ".." => {}
                _ => descended = true,
            }
        }
        if spelling.len() > MAX_DEPENDENCY_PATH_BYTES {
            return Err(DependencyPathReason::TooLong {
                limit: MAX_DEPENDENCY_PATH_BYTES,
                actual: spelling.len(),
            });
        }
        Ok(DependencyPath(spelling.to_string()))
    }

    /// The canonical relative spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The path's segments in order, each either `..` or a directory name.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }
}

/// Why a declared path cannot name a dependency's location.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DependencyPathReason {
    /// The path is absolute; a dependency is located relative to the consuming
    /// root so the two relocate together.
    Absolute,
    /// The path is empty, uses a backslash separator, contains an ASCII control
    /// character, has an empty or `.` segment, or has a `..` segment after a
    /// descending one.
    NonCanonical,
    /// The path is canonical but longer than [`MAX_DEPENDENCY_PATH_BYTES`] UTF-8
    /// bytes.
    TooLong {
        /// The inclusive maximum path length, in UTF-8 bytes.
        limit: usize,
        /// The offending path's length, in UTF-8 bytes.
        actual: usize,
    },
}

/// One declared dependency: the consuming project's alias for it and its relative
/// location. Constructed only by [`Manifest::parse`](crate::Manifest::parse).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Dependency {
    alias: DependencyAlias,
    path: DependencyPath,
}

impl Dependency {
    pub(crate) fn new(alias: DependencyAlias, path: DependencyPath) -> Self {
        Self { alias, path }
    }

    /// The consumer-chosen alias.
    pub fn alias(&self) -> &DependencyAlias {
        &self.alias
    }

    /// The declared relative location.
    pub fn path(&self) -> &DependencyPath {
        &self.path
    }
}
