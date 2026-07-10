//! The identity-root spellings a diagnostic recovers when it renders an interned
//! type leaf. Nominal type leaves carry their identity as an interned id, not a
//! stored source string, so a mismatch recovers the original spelling by id. Almost
//! every spelling is already recorded in [`CheckedFacts`] — a resource's owning
//! module, an enum's module and name — and is read back from there. The one spelling
//! the facts cannot supply is an identity root that names no declared store:
//! `Id(^missing)` resolves to no [`StoreId`], yet its mismatch prose must still read
//! `Id(^missing)`. Those spellings live in the [`StoreRootArena`], the single owned
//! table this recovery adds.

use std::collections::HashMap;
use std::path::PathBuf;

use marrow_syntax::{IdentityTypeExpr, ParsedSource, TypeExpr};

use crate::annotation_refs::{TypeAnnotationBodies, walk_declaration_type_refs};
use crate::driver::resource_type_name;
use crate::facts::{CheckedFacts, EnumId, ResourceId, StoreId};

/// A saved-store root as it appears in an identity type, interned first-wins. A
/// root that names a declared store also has a [`StoreId`]; an undeclared root has
/// only its spelling. The arena keeps every declared root's slot aligned with its
/// store so declared-ness is a lookup, not a second leaf shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StoreRootId(pub u32);

/// The interned spellings of every identity root a program mentions: declared store
/// roots first, in store order, then the undeclared roots named only in annotations,
/// in first-mention order. Two `Id(^missing)` annotations intern to one id, so a
/// mismatch between them reads as one type, matching the pre-interning string.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreRootArena {
    spellings: Vec<String>,
    /// The store a declared root names, at the root's arena slot; `None` for an
    /// undeclared root. Declared roots occupy the leading slots aligned with
    /// [`CheckedFacts::stores`].
    declared: Vec<Option<StoreId>>,
    by_spelling: HashMap<String, StoreRootId>,
}

impl StoreRootArena {
    /// Intern every identity root a program mentions. Declared store roots are
    /// interned first, in store order, so a declared root's arena slot never shifts
    /// when a later annotation adds an undeclared root; the undeclared roots follow
    /// in the order the shared type-annotation walk first reaches them.
    pub(crate) fn build(facts: &CheckedFacts, sources: &HashMap<PathBuf, &ParsedSource>) -> Self {
        let mut arena = Self::default();
        for store in facts.stores() {
            arena.intern(store.root.clone(), Some(store.id));
        }
        for module in facts.modules() {
            let Some(parsed) = sources.get(&module.source_file) else {
                continue;
            };
            for declaration in &parsed.file.declarations {
                walk_declaration_type_refs(declaration, TypeAnnotationBodies::Include, &mut |ty| {
                    intern_identity_roots(&mut arena, ty);
                });
            }
        }
        arena
    }

    /// Build an arena that interns the given declared roots in order, for tests that
    /// render an identity leaf without a full program. Each root is aligned with a
    /// [`StoreId`] at its position, matching how [`Self::build`] lays declared roots
    /// out first.
    #[cfg(test)]
    pub(crate) fn from_declared_roots(roots: &[&str]) -> Self {
        let mut arena = Self::default();
        for (index, root) in roots.iter().enumerate() {
            arena.intern(root.to_string(), Some(StoreId(index as u32)));
        }
        arena
    }

    fn intern(&mut self, spelling: String, declared: Option<StoreId>) -> StoreRootId {
        if let Some(id) = self.by_spelling.get(&spelling) {
            return *id;
        }
        let id = StoreRootId(self.spellings.len() as u32);
        self.by_spelling.insert(spelling.clone(), id);
        self.spellings.push(spelling);
        self.declared.push(declared);
        id
    }

    /// The arena id a root spelling interns to, if the program mentions it. Every
    /// identity leaf's root originates from a declared store or a walked annotation,
    /// so a live leaf's root is always present.
    pub fn id(&self, spelling: &str) -> Option<StoreRootId> {
        self.by_spelling.get(spelling).copied()
    }

    /// The `^root` spelling an arena id names.
    pub fn spelling(&self, id: StoreRootId) -> Option<&str> {
        self.spellings.get(id.0 as usize).map(String::as_str)
    }

    /// The store a declared root names, or `None` when the root names no declared
    /// store. Declared-ness is this lookup off the id, not a second leaf shape.
    pub fn declared_store(&self, id: StoreRootId) -> Option<StoreId> {
        self.declared.get(id.0 as usize).copied().flatten()
    }
}

/// The id-to-spelling recovery a diagnostic renders type-leaf identities through:
/// the interned facts for every declaration, plus the identity-root arena. It
/// borrows both, holding no spelling of its own beyond the arena, so the facts stay
/// the one owner of a resource's or enum's name. Constructed at each emit site from
/// the program's current facts, so it always reflects the latest rebuild.
#[derive(Debug, Clone, Copy)]
pub struct DeclIds<'a> {
    facts: &'a CheckedFacts,
    roots: &'a StoreRootArena,
}

impl<'a> DeclIds<'a> {
    pub(crate) fn new(facts: &'a CheckedFacts, roots: &'a StoreRootArena) -> Self {
        Self { facts, roots }
    }

    /// A resource's module-qualified spelling, or its bare name in a module-less
    /// script, reusing the one owner of that format so the rendered bytes cannot
    /// fork from the name a resource type stored before interning.
    pub fn resource_display(&self, id: ResourceId) -> String {
        let resource = self.facts.resource(id);
        let module = &self.facts.modules()[resource.module.0 as usize];
        resource_type_name(&module.name, &resource.name)
    }

    /// An enum's owning-module name and bare name. A mismatch qualifies the two
    /// sides by comparing these, so the pair is returned rather than a formatted
    /// string.
    pub fn enum_owner_and_name(&self, id: EnumId) -> Option<(&'a str, &'a str)> {
        let enum_fact = self.facts.enum_(id)?;
        let module = self.facts.modules().get(enum_fact.module.0 as usize)?;
        Some((module.name.as_str(), enum_fact.name.as_str()))
    }

    /// The bare `^root` spelling of an identity root, declared or not, for the
    /// `Id(^root)` mismatch form.
    pub fn root_spelling(&self, id: StoreRootId) -> Option<&'a str> {
        self.roots.spelling(id)
    }

    /// The store a declared identity root names, or `None` when the root names no
    /// declared store.
    pub fn declared_store(&self, id: StoreRootId) -> Option<StoreId> {
        self.roots.declared_store(id)
    }

    /// The arena id a root spelling interns to. Every identity leaf's root is
    /// interned at facts-rebuild time, so a live leaf always resolves.
    pub fn root_id(&self, spelling: &str) -> Option<StoreRootId> {
        self.roots.id(spelling)
    }
}

fn intern_identity_roots(arena: &mut StoreRootArena, ty: &TypeExpr) {
    match ty {
        TypeExpr::Identity(IdentityTypeExpr { root, .. }) => {
            let declared = arena
                .by_spelling
                .get(root)
                .and_then(|id| arena.declared_store(*id));
            arena.intern(root.clone(), declared);
        }
        TypeExpr::Sequence { element, .. } => intern_identity_roots(arena, element),
        TypeExpr::Optional { inner, .. } => intern_identity_roots(arena, inner),
        TypeExpr::Name { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_is_first_wins() {
        let mut arena = StoreRootArena::default();
        let first = arena.intern("missing".to_string(), None);
        let again = arena.intern("missing".to_string(), None);
        assert_eq!(first, again);
        assert_eq!(arena.spellings.len(), 1);
        assert_eq!(arena.spelling(first), Some("missing"));
    }

    #[test]
    fn declared_precedes_undeclared_and_recovers_its_store() {
        let mut arena = StoreRootArena::default();
        let books = arena.intern("books".to_string(), Some(StoreId(3)));
        let missing = arena.intern("missing".to_string(), None);
        assert_eq!(books, StoreRootId(0));
        assert_eq!(missing, StoreRootId(1));
        assert_eq!(arena.declared_store(books), Some(StoreId(3)));
        assert_eq!(arena.declared_store(missing), None);
    }

    #[test]
    fn out_of_range_id_recovers_nothing() {
        let arena = StoreRootArena::default();
        assert_eq!(arena.spelling(StoreRootId(0)), None);
        assert_eq!(arena.declared_store(StoreRootId(0)), None);
    }
}
