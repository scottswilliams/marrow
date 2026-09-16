//! The address of one captured source file: the tree it came from and its
//! identity in that tree.
//!
//! A [`FileIdentity`] is root-relative and location-independent, so the same
//! spelling names a different file in each captured tree. Every compiler concept
//! that points at a file — a diagnostic, a declaration site, a generic mint site,
//! an editor fact's target — therefore addresses it by this pair, never by the
//! identity alone. The origin travels beside the identity rather than folded into
//! it: a library keeps one file spelling whether it is compiled standalone or as
//! somebody's dependency.

use marrow_project::{FileIdentity, ModuleInput, SourceOrigin};

/// One captured source file: the tree it came from and its canonical identity
/// relative to that tree's root.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ProjectFile {
    origin: SourceOrigin,
    identity: FileIdentity,
}

impl ProjectFile {
    /// Address the file `identity` names in `origin`.
    pub fn new(origin: SourceOrigin, identity: FileIdentity) -> Self {
        Self { origin, identity }
    }

    /// Address a file of the root project, the origin a single-tree caller means.
    pub fn root(identity: FileIdentity) -> Self {
        Self::new(SourceOrigin::Root, identity)
    }

    /// The tree this file was captured from.
    pub fn origin(&self) -> &SourceOrigin {
        &self.origin
    }

    /// The canonical identity, relative to the root of that tree.
    pub fn identity(&self) -> &FileIdentity {
        &self.identity
    }

    /// The one rendered spelling of this address: the file identity for the root
    /// project, and `alias:identity` for a dependency. A tool or an image string
    /// that must tell two trees' identical file spellings apart renders through
    /// here, so every such rendering agrees. The separator cannot occur in an
    /// alias, which is an identifier, nor in an identity, which is a contained
    /// relative path.
    pub fn spelling(&self) -> String {
        match self.origin.alias() {
            None => self.identity.as_str().to_string(),
            Some(alias) => format!("{}:{}", alias.as_str(), self.identity.as_str()),
        }
    }

    /// The retained variable bytes this address charges: the file spelling plus the
    /// declaring alias, which is a short tag rather than a repeated path.
    pub(crate) fn retained_owned_bytes(&self) -> usize {
        self.identity.as_str().len() + self.origin.alias().map_or(0, |alias| alias.as_str().len())
    }
}

impl From<&ModuleInput> for ProjectFile {
    fn from(module: &ModuleInput) -> Self {
        Self::new(module.origin().clone(), module.identity().clone())
    }
}
