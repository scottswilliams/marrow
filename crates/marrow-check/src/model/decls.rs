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
use crate::facts::{CheckedFacts, StoreId};

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
