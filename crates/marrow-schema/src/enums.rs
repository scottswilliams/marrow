//! The compiled enum shape and the member-path lookups over it: value, `is`,
//! and `match` arm resolution all walk the one [`EnumSchema`] tree.

/// The compiled form of an enum: a named, fixed set of members held flat in
/// pre-order DFS, with the tree shape carried by each member's `parent` link. A
/// member's index is a source traversal handle for tree paths, not durable
/// value identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumSchema {
    pub name: String,
    pub docs: Vec<String>,
    pub members: Vec<EnumMemberSchema>,
}

/// One enum member. `parent` is the traversal index of the enclosing member,
/// `None` at the top level. A `category` member groups its descendants and is not
/// selectable as a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumMemberSchema {
    pub name: String,
    pub docs: Vec<String>,
    pub parent: Option<usize>,
    pub category: bool,
}

/// The outcome of walking a relative member path against an [`EnumSchema`]. The
/// one walk behind value, `is`, and `match` arm resolution returns this so each
/// caller applies its own position rule (selectability) to a single resolved
/// member and reports ambiguity with the same actionable wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberPathResolution {
    /// The path names exactly this member traversal index.
    Found(usize),
    /// A single bare name appears under more than one parent, so it cannot pick
    /// one member. Carries the traversal index of every match, in pre-order, so a
    /// caller can render the disambiguating paths and apply its own position rule
    /// (a value position excludes a category candidate; an `is` operand admits one).
    Ambiguous(Vec<usize>),
    /// No member the path could walk to. Either the first segment is not a member
    /// of the enum, or a later segment is not a child of the member before it.
    NotFound,
}

impl EnumSchema {
    /// Walk a relative member path (`["tiger", "bengal"]`) to a single member. A
    /// qualified path starts at a top-level member and walks parent→child, one
    /// segment per level; since sibling names are unique the walk is always
    /// unambiguous. A bare single name may sit at any depth and is the one position
    /// a duplicate can leave unresolved: the same name under different parents
    /// (`tiger::paw`, `lion::paw`) is [`MemberPathResolution::Ambiguous`].
    ///
    /// The single shared walk behind value, `is`, and `match` arm resolution: each
    /// caller decides whether the resolved member is valid for its position (a value
    /// rejects a category; an `is` operand or an arm admits one). The ambiguity case
    /// carries the qualifying paths so every caller reports the same fix.
    pub fn walk_member_path(&self, path: &[&str]) -> MemberPathResolution {
        let Some((&first, rest)) = path.split_first() else {
            return MemberPathResolution::NotFound;
        };
        // A bare single name searches the whole tree by name: exactly one match
        // resolves, several (the same name under different parents) is ambiguous.
        if rest.is_empty() {
            let matches: Vec<usize> = (0..self.members.len())
                .filter(|&ordinal| self.member_is(ordinal, first))
                .collect();
            return match matches.as_slice() {
                [] => MemberPathResolution::NotFound,
                [ordinal] => MemberPathResolution::Found(*ordinal),
                _ => MemberPathResolution::Ambiguous(matches),
            };
        }
        // A qualified path starts at the top level (`tiger` in `tiger::paw`) and
        // walks children downward. Top-level names are themselves unique only per
        // sibling level, but a qualified path's leading segment must be a top-level
        // member; if several share the name the path cannot pick one.
        let roots: Vec<usize> = (0..self.members.len())
            .filter(|&ordinal| {
                self.members[ordinal].parent.is_none() && self.member_is(ordinal, first)
            })
            .collect();
        let [start] = roots.as_slice() else {
            return MemberPathResolution::NotFound;
        };
        let mut current = *start;
        for &segment in rest {
            match self.child_named(current, segment) {
                Some(child) => current = child,
                None => return MemberPathResolution::NotFound,
            }
        }
        MemberPathResolution::Found(current)
    }

    /// Whether the member at this traversal index has the given name.
    fn member_is(&self, ordinal: usize, name: &str) -> bool {
        self.members.get(ordinal).is_some_and(|m| m.name == name)
    }

    /// The traversal index of the child of `parent` named `name`, if any.
    /// Children are the members whose `parent` link is `parent`; sibling names are
    /// unique, so at most one matches.
    fn child_named(&self, parent: usize, name: &str) -> Option<usize> {
        (0..self.members.len()).find(|&ordinal| {
            self.members[ordinal].parent == Some(parent) && self.members[ordinal].name == name
        })
    }

    /// The member name a traversal index selects, or `None` if it is out of range.
    pub fn member_name(&self, ordinal: usize) -> Option<&str> {
        self.members.get(ordinal).map(|m| m.name.as_str())
    }

    /// Whether one traversal index sits at or under `ancestor` in the member tree
    /// — the `is` primitive. Inclusive: a member is its own descendant, so a
    /// concrete leaf on both sides is exact equality and a category ancestor
    /// matches its whole subtree.
    pub(crate) fn is_descendant(&self, ordinal: usize, ancestor: usize) -> bool {
        let mut current = Some(ordinal);
        while let Some(index) = current {
            if index == ancestor {
                return true;
            }
            current = self.members.get(index).and_then(|member| member.parent);
        }
        false
    }

    /// Whether the member at this traversal index is a category — a grouping node
    /// that is not selectable as a value but may name a whole subtree in `match`
    /// or `is`.
    pub fn is_category(&self, ordinal: usize) -> bool {
        self.members.get(ordinal).is_some_and(|m| m.category)
    }

    /// The traversal indices at or under `ancestor`, inclusive — the members a
    /// category arm or an `is` test covers.
    pub fn subtree_ordinals(&self, ancestor: usize) -> impl Iterator<Item = usize> + '_ {
        (0..self.members.len()).filter(move |&ordinal| self.is_descendant(ordinal, ancestor))
    }

    /// The traversal indices for concrete leaves. A category is never selectable,
    /// and a member with children is a grouping node whose value is one of its
    /// descendants, so only childless non-category members are selectable.
    pub fn selectable_leaves(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.members.len()).filter(move |&ordinal| self.is_selectable_leaf(ordinal))
    }

    /// Whether a traversal index is a selectable leaf: concrete (not a category)
    /// and with no children.
    pub fn is_selectable_leaf(&self, ordinal: usize) -> bool {
        let Some(member) = self.members.get(ordinal) else {
            return false;
        };
        !member.category && !self.has_children(ordinal)
    }

    /// Whether any member names this traversal index as its parent.
    pub fn has_children(&self, ordinal: usize) -> bool {
        self.members.iter().any(|m| m.parent == Some(ordinal))
    }

    /// The dotted path of names from the root to `ordinal` (`["tiger", "bengal"]`),
    /// for diagnostics. Empty when the traversal index is out of range.
    pub fn member_path(&self, ordinal: usize) -> Vec<&str> {
        let mut path = Vec::new();
        let mut current = Some(ordinal);
        while let Some(index) = current {
            let Some(member) = self.members.get(index) else {
                break;
            };
            path.push(member.name.as_str());
            current = member.parent;
        }
        path.reverse();
        path
    }
}
