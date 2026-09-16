//! Origin-scoped addressing: the tree one captured file came from and its identity
//! in that tree, and the tree one declared name is written in.
//!
//! A [`FileIdentity`] is root-relative and location-independent, so the same
//! spelling names a different file in each captured tree. Every compiler concept
//! that points at a file — a diagnostic, a declaration site, a generic mint site,
//! an editor fact's target — therefore addresses it by this pair, never by the
//! identity alone. The origin travels beside the identity rather than folded into
//! it: a library keeps one file spelling whether it is compiled standalone or as
//! somebody's dependency.

use marrow_project::{FileIdentity, ModuleInput, ProjectInput, SourceOrigin};

use crate::decl::DeclarationSite;

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

/// The trees one compilation captured, in canonical order: the root first, then
/// each dependency under the alias the consuming project declares it by.
///
/// The one authority for what an alias-rooted first segment means. A name's origin
/// is never recovered by reading its spelling: a resolver asks this set whether a
/// segment is a declared alias and takes the origin it holds.
#[derive(Clone, Debug)]
pub(crate) struct CapturedOrigins(Vec<SourceOrigin>);

impl CapturedOrigins {
    /// The origins `project` was captured from.
    pub(crate) fn of(project: &ProjectInput) -> Self {
        Self(project.origins().to_vec())
    }

    /// The dependency origin declared under `alias`, or `None` when the segment
    /// names no declared dependency.
    pub(crate) fn declared(&self, alias: &str) -> Option<&SourceOrigin> {
        self.0
            .iter()
            .find(|origin| origin.alias().is_some_and(|held| held.as_str() == alias))
    }
}

/// One declared name in the tree that declares it: a type name, a store-root
/// placement, the resource spelling of a Product, or a qualified path under one.
///
/// Declared namespaces are origin-scoped: two captured trees may each declare
/// `Book` or `^books`, and neither answers the other's name. The pair is always
/// built from an origin and a name — through [`Self::written`] when the name is
/// read from source spelling — and an origin is never recovered from a spelling.
///
/// This is one key *shape*, not one namespace. Each ledger keyed by it holds a
/// namespace of its own, discriminated by the ledger rather than by the key, so a
/// type name and a durable name of one spelling never meet.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct ScopedName {
    origin: SourceOrigin,
    name: String,
}

impl ScopedName {
    pub(crate) fn new(origin: &SourceOrigin, name: &str) -> Self {
        Self {
            origin: origin.clone(),
            name: name.to_string(),
        }
    }

    /// Where a written type spelling resolves from `origin`, given the captured
    /// trees. A bare name resolves in the tree that wrote it; a two-segment
    /// `alias::Name` resolves in the dependency the alias declares; any other shape
    /// names no tree.
    pub(crate) fn written(
        origins: &CapturedOrigins,
        origin: &SourceOrigin,
        written: &str,
    ) -> Option<Self> {
        let mut segments = marrow_syntax::type_name_segments(written);
        let first = segments.next()?;
        let Some(name) = segments.next() else {
            return Some(Self::new(origin, first));
        };
        if segments.next().is_some() {
            return None;
        }
        origins
            .declared(first)
            .map(|declaring| Self::new(declaring, name))
    }

    /// The name one declaration takes, scoped to the tree it is written in.
    pub(crate) fn declared(site: &DeclarationSite<'_>) -> Self {
        Self::new(site.file.origin(), site.name)
    }

    /// The tree this name is declared in.
    pub(crate) fn origin(&self) -> &SourceOrigin {
        &self.origin
    }

    /// The bare name, as the declaring tree's own source spells it.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// The same tree's name for one step below this one, `self.name` extended by
    /// `step`: a resource record's `Record.group` anchor for an unkeyed group's
    /// leaves, or the qualified constructor path of a branch under a resource or
    /// branch. It is the spelling the image carries for that node, and no
    /// declaration can take it — a declared name has no dot — so a nested name and
    /// a top-level one of the same spelling never share a key.
    pub(crate) fn below(&self, step: &str) -> Self {
        Self {
            origin: self.origin.clone(),
            name: format!("{}.{step}", self.name),
        }
    }
}
