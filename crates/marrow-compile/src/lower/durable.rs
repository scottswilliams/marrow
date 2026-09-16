//! The durable-place model and the lowering of durable reads, writes, presence,
//! traversal, and managed-index access.

use super::*;

mod mutation_emit;

/// The structural durable shape of a place expression.
pub(super) enum DurShape {
    Entry,
    Field,
}

/// How one key column of a durable operation's key-path reaches the stack.
#[derive(Clone, Copy)]
pub(super) enum PlaceKey<'e> {
    /// A key operand expression, lowered — and therefore evaluated — at the
    /// operation site (the inline `^root(key)` form).
    Expr(&'e Expression),
    /// A key already evaluated once into a local slot (a named `place`); each use reads
    /// the slot, so the operand runs exactly once however many operations use the place.
    Bound(u16),
    /// The whole root key-path supplied by one entry-identity operand (`^root[id]`):
    /// `IdentityKeyPath` spreads it into the root's `cols` key columns. One `Identity`
    /// key stands for every root key column, so it is only the root whole-key form and
    /// never mixes with per-column keys.
    Identity {
        expr: &'e Expression,
        root: RootId,
        cols: u16,
    },
}

/// One column of a durable operation's key-path: how it reaches the stack and its
/// scalar type. A single-key root entry is a one-column path `[root_key]`; a
/// single-level branch entry is a two-column path `[root_key, branch_key]`. A
/// composite-key root has several root key columns rather than one. Column order is
/// owned by [`FnLowerer::emit_key_path`].
#[derive(Clone, Copy)]
pub(super) struct DurKey<'e> {
    pub(super) key: PlaceKey<'e>,
    pub(super) key_ty: ScalarType,
}

/// A resolved durable place: the key-path that addresses its node and its target. The
/// path columns are inline operand expressions or a source-local `place`'s
/// pre-evaluated slots; the target is the whole entry or one field.
pub(super) struct DurablePlace<'a, 'e> {
    keys: Vec<DurKey<'e>>,
    target: DurTarget<'a>,
    /// The family of the entry the operation addresses: the node's own for a whole
    /// entry or a field, the root's for a group or a group leaf.
    pub(super) family: &'a Family,
    span: SourceSpan,
}

impl DurablePlace<'_, '_> {
    /// This place's whole key-path as pre-evaluated slots (root-first) when *every*
    /// column is a `Bound` slot — the shape a present-form operation and a place-entry
    /// presence guard require. `None` if any column is an inline key expression.
    pub(super) fn bound_key_path(&self) -> Option<Vec<u16>> {
        self.keys
            .iter()
            .map(|column| match column.key {
                PlaceKey::Bound(slot) => Some(slot),
                PlaceKey::Expr(_) | PlaceKey::Identity { .. } => None,
            })
            .collect()
    }
}

/// A source-local named `place`: a durable entry designation whose key columns were
/// evaluated exactly once into `key_slots` at the binding. Whole-entry and field
/// operations through the place read those slots rather than re-evaluating the key
/// operands.
///
/// The place retains the exact durable node it was bound against, so every later
/// operation through it addresses that occurrence directly. A branch entry record and a
/// resource spelling are Product *declaration* facts that several roots may project, so
/// recovering the node from one of them would answer with whichever root came first.
pub(super) struct PlaceLocal<'a> {
    pub(super) name: String,
    pub(super) key_slots: Vec<(u16, ScalarType)>,
    pub(super) node: DurNode<'a>,
}

/// A resolved source managed-index read `^root.index[keys]`: the index, the executable
/// root that owns it (whose identity backs a scan's yielded `Id(^root)`), and the bracket
/// key operands. The index and root borrow the durable registry (lifetime `'a`); the
/// operands borrow the source expression (lifetime `'e`).
pub(super) struct IndexRead<'a, 'e> {
    pub(super) index: &'a crate::durable::DurableIndex,
    pub(super) root: &'a crate::durable::DurableRoot,
    pub(super) keys: &'e [Expression],
}

impl<'a> PlaceLocal<'a> {
    /// The entry this place addresses as a presence fact's key: its family and its whole
    /// key-path as pre-evaluated slots (root-first).
    pub(super) fn fact_key(&self) -> (&'a Family, Vec<u16>) {
        (
            self.node.family(),
            self.key_slots.iter().map(|(slot, _)| *slot).collect(),
        )
    }

    /// This place's key-path as resolved [`DurKey`] columns reading the pre-evaluated
    /// slots, root column first.
    pub(super) fn bound_keys(&self) -> Vec<DurKey<'static>> {
        self.key_slots
            .iter()
            .map(|(slot, ty)| DurKey {
                key: PlaceKey::Bound(*slot),
                key_ty: *ty,
            })
            .collect()
    }
}

/// A resolved durable target: the whole entry, one field, a whole root-level group, or
/// one group leaf.
///
/// Each site-bearing variant carries the **handle** its site was bound to, not a minted
/// operand: the binding happens once, where the member or place is resolved, and the
/// operand is minted at the instruction that names it.
#[derive(Clone)]
enum DurTarget<'a> {
    /// A whole durable entry, addressed through the exact durable node the resolver
    /// walked to. The node carries the occurrence and the materialized record, `handle`
    /// the whole-payload site bound over it, so neither is resolved back later.
    Entry {
        node: DurNode<'a>,
        handle: OccurrenceSiteHandle,
    },
    Field {
        handle: OccurrenceSiteHandle,
        /// The field's value type (a scalar or a widened composite), from which the
        /// read result and written-value type are built.
        ty: GArg,
        required: bool,
    },
    /// A whole root-level `group` (`^root(k).group`): read, replaced, or erased as one
    /// materialized `record` value through the `GroupEntry` site `handle` binds.
    Group {
        handle: OccurrenceSiteHandle,
        record: TypeId,
        /// Whether a leaf of the group is `required`: such a group is part of every
        /// present entry and is erased only with it.
        holds_required: bool,
    },
    /// One leaf of a root-level group (`^root(k).group.leaf`). A read materializes the
    /// whole group through the group's `GroupEntry` site and projects `slot`; a write or
    /// clear is a whole-group read-modify-write, so a leaf never has a site of its own.
    GroupLeaf {
        handle: OccurrenceSiteHandle,
        slot: u16,
        ty: GArg,
        required: bool,
    },
}

/// A node reached along a resolved durable entry address: the root, or a keyed branch on
/// the address's branch chain. Both expose the same navigation, so the recursive address
/// resolver walks them uniformly at any depth.
///
/// A branch carries the root occurrence it was reached through as well as the branch
/// itself: a site is occurrence-qualified, and a branch belongs to a declaration several
/// roots may project, so the branch alone cannot name one.
#[derive(Clone, Copy)]
pub(super) enum DurNode<'a> {
    Root(&'a crate::durable::DurableRoot),
    Branch {
        root: &'a crate::durable::DurableRoot,
        branch: &'a crate::durable::DurableBranch,
    },
}

impl<'a> DurNode<'a> {
    /// The root occurrence this node was reached through; every site over the node is
    /// bound against it.
    pub(super) fn occurrence(&self) -> &'a RootOccurrenceSelector {
        match self {
            DurNode::Root(root) | DurNode::Branch { root, .. } => &root.occurrence,
        }
    }

    /// The canonical declaration path of this node's own keyed placement — the path its
    /// whole-payload site is bound at.
    pub(super) fn entry_path(&self) -> &'a CanonicalDeclarationPathSelector {
        match self {
            DurNode::Root(root) => &root.placement,
            DurNode::Branch { branch, .. } => &branch.path,
        }
    }

    fn record(&self) -> TypeId {
        match self {
            DurNode::Root(root) => root.record,
            DurNode::Branch { branch, .. } => branch.record,
        }
    }

    /// The entry family this node belongs to.
    pub(super) fn family(&self) -> &'a Family {
        match self {
            DurNode::Root(root) => &root.family,
            DurNode::Branch { branch, .. } => &branch.family,
        }
    }

    /// The root occurrence's own descriptor, so a branch hop below this node keeps naming
    /// the occurrence the address was rooted at.
    fn root(&self) -> &'a crate::durable::DurableRoot {
        match self {
            DurNode::Root(root) | DurNode::Branch { root, .. } => root,
        }
    }

    pub(super) fn branch(&self, name: &str) -> Option<&'a crate::durable::DurableBranch> {
        match self {
            DurNode::Root(root) => root.branch(name),
            DurNode::Branch { branch, .. } => branch.branch(name),
        }
    }

    /// This node extended by the keyed branch `branch`, keeping the root occurrence.
    pub(super) fn child(&self, branch: &'a crate::durable::DurableBranch) -> DurNode<'a> {
        DurNode::Branch {
            root: self.root(),
            branch,
        }
    }

    /// This node's field `name`, whether the node is the root entry or a keyed branch
    /// entry. The caller binds and deduplicates the field's operation site when it builds
    /// the field target, so an untouched field mints no site.
    fn field(&self, name: &str) -> Option<&'a crate::durable::DurableField> {
        match self {
            DurNode::Root(root) => root.field(name),
            DurNode::Branch { branch, .. } => branch.field(name),
        }
    }

    fn name(&self) -> &str {
        match self {
            DurNode::Root(root) => &root.name,
            DurNode::Branch { branch, .. } => &branch.name,
        }
    }

    /// The member-ledger owner whose declared members back this node's fields: the
    /// resource record a root materializes. A branch owns no member ledger — its fields
    /// are the keyed layer, where a refused member refuses the whole root.
    fn member_owner(&self) -> Option<&ScopedName> {
        match self {
            DurNode::Root(root) => Some(&root.resource),
            DurNode::Branch { .. } => None,
        }
    }

    fn no_field_message(&self, field: &str) -> String {
        match self {
            DurNode::Root(root) => format!("`{}` has no field `{field}`", root.name),
            DurNode::Branch { branch, .. } => {
                format!("branch `{}` has no field `{field}`", branch.name)
            }
        }
    }

    pub(super) fn no_branch_message(&self, branch: &str) -> String {
        format!("`{}` has no keyed branch `{branch}`", self.name())
    }
}

/// A resolved durable traversal place: the traversed layer's whole-entry site, the
/// immediate key type it enumerates, and the ancestor key-path locating its parent entry
/// (empty for a root family, `[root_key]` for a single-level branch family). The bounded
/// traversal opcode pushes the ancestor path root-first, then the optional inclusive
/// `from` key, and freezes the traversed layer's immediate keys.
pub(super) struct TraversalTarget<'a, 'e> {
    /// The exact durable node of the traversed layer — a store root or a keyed branch.
    /// It carries the layer's whole-entry site and the materialized record a two-binding
    /// traversal's per-iteration address pin (`for k, p in …`) binds `p` over, and it is
    /// the node that pin retains.
    pub(super) node: DurNode<'a>,
    pub(super) key_ty: ScalarType,
    pub(super) ancestor_keys: Vec<DurKey<'e>>,
    pub(super) span: SourceSpan,
}

/// Whether an instruction is a direct durable-place operation — a read, write, presence
/// probe, erase, or managed-index access over a `^` place. Tests must reach these
/// through calls.
pub(crate) fn is_durable_place_op(instr: &Instr) -> bool {
    matches!(
        instr.op_class(),
        OpClass::DurableMutation | OpClass::DurableRead
    )
}

/// Whether an instruction stages a durable mutation (a write, replacement, or erase) —
/// the sites the requires-ambient-transaction check demands a transaction for.
pub(crate) fn is_mutation_instr(instr: &Instr) -> bool {
    instr.op_class() == OpClass::DurableMutation
}

/// Where a durable address is rooted.
///
/// The two spellings differ only at the base: an inline address names its store at the
/// `^name` leaf, a place-rooted one starts at an in-scope `place`/pin binding whose key
/// columns were evaluated once. Every selector below the base resolves the same way, so
/// this is the only thing the shared resolvers branch on.
#[derive(Clone, Copy)]
pub(super) enum EntryBase<'a> {
    Inline(&'a crate::durable::DurableRoot),
    Place,
}

impl<'a, 'd> FnLowerer<'a, 'd> {
    // --- Durable places ---

    /// Detect the inline durable shape of a place expression: a whole-entry address
    /// `^root(key)….b(bkey)` at any depth, or a field-exact address
    /// `<entry-address>.field`. No diagnostics, and it does not see source-local `place`
    /// bindings; use [`Self::durable_access`] for the full detection.
    pub(super) fn durable_shape(expr: &Expression) -> Option<DurShape> {
        if is_entry_address(expr) {
            Some(DurShape::Entry)
        } else if is_field_address(expr) || is_group_leaf_address(expr) {
            // A field-exact address or a whole root-level group (both `<entry>.name`), or
            // a group leaf one field selection deeper. The resolver disambiguates a group
            // from a field by name.
            Some(DurShape::Field)
        } else {
            None
        }
    }

    /// The inline durable ^-address shape of `expr`, confirming a group-leaf address
    /// against the resolved durable model: [`Self::durable_shape`] recognizes
    /// `<entry>.mid.leaf` syntactically, but `mid` must actually name a root-level group.
    /// A `mid` that is a stored field (or an unknown name) leaves the expression an
    /// ordinary field projection, diagnosed by the ordinary field path rather than
    /// compiling to a codeless durable body.
    pub(super) fn durable_shape_here(
        &self,
        expr: &Expression,
    ) -> Result<Option<DurShape>, DeclarationIndexDrift> {
        if is_group_leaf_address(expr) {
            let Expression::Field { base, .. } = expr else {
                return Ok(None);
            };
            return Ok(self.names_a_group(base)?.then_some(DurShape::Field));
        }
        Ok(Self::durable_shape(expr))
    }

    /// The durable node an entry address reaches, without emitting.
    ///
    /// One walker for both spellings: an inline `^root[k].b[bk]…` chain resolved against
    /// the named store root, and a place-rooted `p.b[bk]…` chain resolved against the node
    /// the `place`/pin binding already addresses. `None` when `expr` is not a resolvable
    /// entry address. The emitting resolvers own the diagnostics; this only classifies.
    ///
    /// Borrows the registry (`'a`), not `&self`.
    pub(super) fn entry_node(
        &self,
        expr: &Expression,
    ) -> Result<Option<DurNode<'a>>, DeclarationIndexDrift> {
        match expr {
            // A bare `place`/pin name.
            Expression::Name { segments, .. } => Ok(match &segments[..] {
                [name] => self.lookup_place(name.text()).map(|place| place.node),
                _ => None,
            }),
            Expression::Keyed { base, .. } => match &**base {
                Expression::SavedRoot { name, .. } => Ok(self
                    .durable
                    .root_by_name(&self.scoped_name(name))?
                    .map(DurNode::Root)),
                Expression::Field {
                    base: parent,
                    name: branch,
                    ..
                } => Ok(match self.entry_node(parent)? {
                    Some(parent) => parent.branch(branch).map(|found| parent.child(found)),
                    None => None,
                }),
                _ => Ok(None),
            },
            _ => Ok(None),
        }
    }

    /// Whether `expr` is the group address `<entry>.group` of a group-leaf address —
    /// distinguishing a group leaf `<entry>.group.leaf` (a durable cell) from a projection
    /// on a durable struct field value `<entry>.field.sub`. Only a root node offers
    /// groups; a nested branch has none.
    fn names_a_group(&self, expr: &Expression) -> Result<bool, DeclarationIndexDrift> {
        let Expression::Field { base, name, .. } = expr else {
            return Ok(false);
        };
        Ok(
            matches!(self.entry_node(base)?, Some(DurNode::Root(root)) if root.group(name).is_some()),
        )
    }

    /// The most recent in-scope `place` binding named `name`, if any.
    pub(super) fn lookup_place(&self, name: &str) -> Option<&PlaceLocal<'a>> {
        self.places.iter().rev().find(|place| place.name == name)
    }

    /// Whether `name` names an in-scope `place`.
    pub(super) fn is_place_name(&self, expr: &Expression) -> bool {
        matches!(
            expr,
            Expression::Name { segments, .. }
                if matches!(&segments[..], [name] if self.lookup_place(name.text()).is_some())
        )
    }

    /// Resolve `^root.index[keys]` or bare `^root.index` through the declared index owner.
    /// A bare read borrows an empty operand slice; each consumer checks its own arity and
    /// index kind. The index reference lives as long as the durable registry, so it may be
    /// held across a mutable lowering call.
    pub(super) fn resolve_index_read<'e>(
        &self,
        expr: &'e Expression,
    ) -> Result<Option<IndexRead<'a, 'e>>, DeclarationIndexDrift> {
        let (base, keys): (&Expression, &[Expression]) = match expr {
            Expression::Keyed { base, keys, .. } => (base.as_ref(), keys.as_slice()),
            _ => (expr, &[]),
        };
        let Expression::Field {
            base: field_base,
            name,
            ..
        } = base
        else {
            return Ok(None);
        };
        let Expression::SavedRoot {
            name: root_name, ..
        } = field_base.as_ref()
        else {
            return Ok(None);
        };
        let root_key = self.scoped_name(root_name);
        let durable: &'a DurableRegistry = self.durable;
        Ok(durable
            .root_by_name(&root_key)?
            .and_then(|root| root.index(name).map(|index| (root, index)))
            .map(|(root, index)| IndexRead { index, root, keys }))
    }

    /// Lower a unique index's exact lookup `^root.index[keys]`: check the operands against
    /// the whole projection, then emit `DurIndexLookup`. The result is the optional source
    /// identity `Id(^root)?` — present with the matching entry's identity, or absent — which
    /// an `if const` head unwraps to a bare `Id(^root)`.
    pub(super) fn lower_index_lookup(
        &mut self,
        root: &'a crate::durable::DurableRoot,
        index: &'a crate::durable::DurableIndex,
        keys: &[Expression],
        span: SourceSpan,
    ) -> ConstructResult<LTy> {
        if keys.len() != index.projection.len() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                format!(
                    "unique index `{}` is looked up by its whole projection of {} key(s)",
                    index.name,
                    index.projection.len()
                ),
            ));
            return Err(LoweringFailure::Recoverable);
        }
        let site = self
            .bind_index_site(root, index)
            .ok_or(LoweringFailure::Recoverable)?;
        // The projection scalar types are copied out first so the operand lowering (a
        // mutable borrow of `self`) does not overlap the index borrow.
        let projection: Vec<ScalarType> = index.projection.clone();
        for (key, key_ty) in keys.iter().zip(&projection) {
            self.lower_as(key, LTy::bare_scalar(*key_ty))?;
        }
        self.push(Instr::DurIndexLookup(site), span)?;
        Ok(LTy::Identity {
            root: root.root_id,
            optional: true,
        })
    }

    /// Lower a unique index's presence probe `exists(^root.index[keys])`: the presence half
    /// of [`lower_index_lookup`], over the same lookup site, without materializing the
    /// found identity.
    pub(super) fn lower_index_exists(
        &mut self,
        root: &'a crate::durable::DurableRoot,
        index: &'a crate::durable::DurableIndex,
        keys: &[Expression],
        span: SourceSpan,
    ) -> ConstructResult<LTy> {
        if keys.len() != index.projection.len() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                format!(
                    "unique index `{}` is probed by its whole projection of {} key(s)",
                    index.name,
                    index.projection.len()
                ),
            ));
            return Err(LoweringFailure::Recoverable);
        }
        let site = self
            .bind_index_site(root, index)
            .ok_or(LoweringFailure::Recoverable)?;
        // The projection scalar types are copied out first so the operand lowering (a
        // mutable borrow of `self`) does not overlap the index borrow.
        let projection: Vec<ScalarType> = index.projection.clone();
        for (key, key_ty) in keys.iter().zip(&projection) {
            self.lower_as(key, LTy::bare_scalar(*key_ty))?;
        }
        self.push(Instr::DurIndexExists(site), span)?;
        Ok(LTy::bare_scalar(ScalarType::Bool))
    }

    pub(super) fn durable_access(
        &self,
        expr: &Expression,
    ) -> Result<Option<DurShape>, DeclarationIndexDrift> {
        if let Some(shape) = self.durable_shape_here(expr)? {
            return Ok(Some(shape));
        }
        // A place-rooted composed address extends a named `place`/pin with the same field,
        // group, and branch selectors an inline `^root` address takes, and classifies the
        // same way: a whole entry, or a field cell. A projection on a durable field
        // *value* (`p.field.sub`) is not a durable cell and falls through to ordinary
        // projection, exactly as the inline form does.
        Ok(match expr {
            Expression::Name { .. } => self.is_place_name(expr).then_some(DurShape::Entry),
            // A place-rooted keyed selection `<place>(.branch[bk])+` is a branch entry.
            Expression::Keyed { .. } => self.is_place_rooted(expr).then_some(DurShape::Entry),
            // A field cell off a place: a stored field, a whole root-level group, or a
            // group leaf. The entry base is classified syntactically, not by resolving it,
            // so an unknown branch there reaches the resolver and reports "no keyed
            // branch" rather than a confusing projection error. A group leaf is confirmed
            // against the model so `p.field.sub` still falls through.
            Expression::Field { base, .. } => (self.names_a_group(base)?
                || (matches!(&**base, Expression::Name { .. } | Expression::Keyed { .. })
                    && self.is_place_rooted(base)))
            .then_some(DurShape::Field),
            _ => None,
        })
    }

    /// Whether the leftmost base of a durable path expression is an in-scope named
    /// `place`/pin — a bare place name, or a place extended by `.field`, `.group[.leaf]`,
    /// or `.branch[bk]` hops.
    fn is_place_rooted(&self, expr: &Expression) -> bool {
        match expr {
            Expression::Name { .. } => self.is_place_name(expr),
            Expression::Field { base, .. } | Expression::Keyed { base, .. } => {
                self.is_place_rooted(base)
            }
            _ => false,
        }
    }

    /// Report a selector that names no leaf of `group`, steering to the leaf's own
    /// refusal when the group declared it and the compiler refused it.
    ///
    /// The group's anchor `Resource.group` is the leaf ledger's owner. Both the inline
    /// `^root(k).group.leaf` address and its place-rooted twin land here.
    fn report_missing_group_leaf(
        &mut self,
        root: &crate::durable::DurableRoot,
        group: &crate::durable::DurableGroup,
        field_name: &str,
        name_span: SourceSpan,
    ) {
        let owner = root.resource.below(&group.name);
        if self.steer_refused_member(&owner, field_name, name_span) {
            return;
        }
        self.fail(SourceDiagnostic::at(
            Code::CheckType,
            self.file,
            name_span,
            format!("group `{}` has no field `{field_name}`", group.name),
        ));
    }

    /// Report a selector that names no member of `node`.
    ///
    /// A member the compiler refused is declared: it left the record's accepted set but
    /// keeps its name, so the address is steered to the refusal rather than told the node
    /// has no such field.
    fn report_missing_member(
        &mut self,
        node: &DurNode<'a>,
        field_name: &str,
        name_span: SourceSpan,
    ) {
        if node
            .member_owner()
            .is_some_and(|owner| self.steer_refused_member(owner, field_name, name_span))
        {
            return;
        }
        self.fail(SourceDiagnostic::at(
            Code::CheckType,
            self.file,
            name_span,
            node.no_field_message(field_name),
        ));
    }

    /// Bind the whole-payload site of the durable entry `node` addresses, against the root
    /// occurrence the address was rooted at.
    fn bind_entry_site(&mut self, node: DurNode<'a>) -> Option<OccurrenceSiteHandle> {
        self.bind_site(
            node.occurrence(),
            node.entry_path(),
            SemanticTarget::WholePayload,
        )
    }

    /// The whole-payload operand of the entry `node` addresses. The durable build already
    /// requested every keyed placement's site, so this returns the id minted there.
    pub(super) fn entry_site_operand(&mut self, node: DurNode<'a>) -> Option<PlannedSiteRef> {
        let handle = self.bind_entry_site(node)?;
        self.site_operand(&handle)
    }

    /// Bind one stored field's leaf site and mint it here, where the field is resolved.
    ///
    /// Site ids are assigned in request order, and a key operand lowered between the
    /// resolution and the instruction may itself address a field — `^r[^r2[1].x].f` lowers
    /// `^r2[1].x` after `^r…f` is resolved and before it is emitted — so minting at the
    /// emission boundary would reorder the site table. The operand is deliberately not
    /// retained: the target holds the handle and the emission re-requests this id.
    fn bind_field_site(
        &mut self,
        node: DurNode<'a>,
        path: &CanonicalDeclarationPathSelector,
    ) -> Option<OccurrenceSiteHandle> {
        let handle = self.bind_site(node.occurrence(), path, SemanticTarget::FieldLeaf)?;
        self.site_operand(&handle)?;
        Some(handle)
    }

    /// Bind the `GroupEntry` site of the root-level group `group` under the root
    /// occurrence `root` — the site every whole-group and group-leaf operation addresses.
    fn bind_group_site(
        &mut self,
        root: &'a crate::durable::DurableRoot,
        group: &'a crate::durable::DurableGroup,
    ) -> Option<OccurrenceSiteHandle> {
        self.bind_site(&root.occurrence, &group.path, SemanticTarget::GroupEntry)
    }

    /// Bind the read site of the managed index `index` of the root occurrence `root`: a
    /// complete-key lookup for a unique index, a progressive-prefix scan otherwise.
    pub(super) fn bind_index_site(
        &mut self,
        root: &'a crate::durable::DurableRoot,
        index: &'a crate::durable::DurableIndex,
    ) -> Option<PlannedSiteRef> {
        let target = if index.unique {
            SemanticTarget::IndexLookup
        } else {
            SemanticTarget::IndexScan
        };
        self.resolve_site(&root.occurrence, &index.path, target)
    }

    /// Emit one key column of a durable operation: lower the inline key expression
    /// (evaluating it here) or read the `place`'s pre-evaluated key slot.
    fn emit_key(
        &mut self,
        key: PlaceKey,
        key_ty: ScalarType,
        span: SourceSpan,
    ) -> ConstructResult<()> {
        match key {
            PlaceKey::Expr(expr) => self.lower_as(expr, LTy::bare_scalar(key_ty)),
            PlaceKey::Bound(slot) => {
                self.push(Instr::LocalGet(slot), span)?;
                Ok(())
            }
            // The one `Identity` key supplies the whole root key-path, so this pushes
            // every root key column, matching the entry site's key arity.
            PlaceKey::Identity { expr, root, cols } => {
                self.lower_as(
                    expr,
                    LTy::Identity {
                        root,
                        optional: false,
                    },
                )?;
                self.push(Instr::IdentityKeyPath(cols), span)?;
                Ok(())
            }
        }
    }

    /// Emit a durable operation's whole key-path, root column first, so the innermost key
    /// is left on top — the order the kernel's `pop_key_path` reads back to a root-first
    /// path. Path length does not name the node kind: a composite-key root is itself
    /// multi-column.
    pub(super) fn emit_key_path(
        &mut self,
        keys: &[DurKey],
        span: SourceSpan,
    ) -> ConstructResult<()> {
        for column in keys {
            self.emit_key(column.key, column.key_ty, span)?;
        }
        Ok(())
    }

    /// Capture the root key-path an entry identity supplies into one pre-evaluated slot
    /// per root key column (root-first). A whole-entry write through it reads and writes
    /// off the same columns several times, so the identity is evaluated once here and the
    /// slots reused, exactly as an inline key tuple is captured.
    pub(super) fn capture_identity_key_slots(
        &mut self,
        expr: &Expression,
        root: RootId,
        cols: u16,
        span: SourceSpan,
    ) -> ConstructResult<Vec<u16>> {
        self.lower_as(
            expr,
            LTy::Identity {
                root,
                optional: false,
            },
        )?;
        self.push(Instr::IdentityKeyPath(cols), span)?;
        let cols = cols as usize;
        // `IdentityKeyPath` leaves the columns root-first, so the last column is on top;
        // pop into slots from the last column back so each slot holds its own column.
        let mut slots = vec![0u16; cols];
        for column in (0..cols).rev() {
            let slot = self
                .alloc_slot(expr.span())
                .ok_or(LoweringFailure::Recoverable)?;
            self.push(Instr::LocalSet(slot), span)?;
            slots[column] = slot;
        }
        Ok(slots)
    }

    /// Capture an entry identity into one pre-evaluated `(slot, scalar)` column per root
    /// key column (root-first). The single owner for recording an identity operand as a
    /// place/traversal key-path, so a place binding and a traversal ancestor spread it
    /// identically.
    pub(super) fn capture_identity_key_columns(
        &mut self,
        expr: &Expression,
        root: RootId,
        cols: u16,
        span: SourceSpan,
    ) -> ConstructResult<Vec<(u16, ScalarType)>> {
        let slots = self.capture_identity_key_slots(expr, root, cols, span)?;
        // The RootId was resolved from a root in this registry when the identity column was
        // built, so it is present here.
        #[expect(
            clippy::expect_used,
            reason = "lowering invariant: an identity operand's RootId names a root in this registry"
        )]
        let scalars = self
            .durable
            .root_by_id(root)
            .expect("an identity operand's root is registered")
            .key
            .clone();
        Ok(slots.into_iter().zip(scalars).collect())
    }

    /// Materialize a durable operation's whole key-path into one pre-evaluated slot per
    /// physical key column (root-first) — the capture a read-modify-write or an upsert
    /// needs so its several ops key off one evaluation. A `Bound` column reuses the place
    /// slot it already holds; an `Expr` column is evaluated once into a fresh slot; an
    /// entry-identity column spreads into one slot per root key column.
    fn capture_key_slots(
        &mut self,
        keys: &[DurKey],
        span: SourceSpan,
    ) -> ConstructResult<Vec<u16>> {
        let mut slots = Vec::with_capacity(keys.len());
        for column in keys {
            match column.key {
                PlaceKey::Bound(slot) => slots.push(slot),
                PlaceKey::Expr(expr) => {
                    let slot = self
                        .alloc_slot(expr.span())
                        .ok_or(LoweringFailure::Recoverable)?;
                    self.emit_key(column.key, column.key_ty, span)?;
                    self.push(Instr::LocalSet(slot), span)?;
                    slots.push(slot);
                }
                PlaceKey::Identity { expr, root, cols } => {
                    slots.extend(self.capture_identity_key_slots(expr, root, cols, span)?);
                }
            }
        }
        Ok(slots)
    }

    /// Lower `place name = ^root(key)`: evaluate the entry address's key tuple exactly
    /// once into a fresh local slot and record the binding. The binding is immutable and
    /// does not shadow an existing name; the target must be a whole durable entry address
    /// (not a field, another place, or a non-durable value).
    pub(super) fn lower_place_binding(
        &mut self,
        name: &str,
        name_span: SourceSpan,
        place_expr: &Expression,
    ) -> ConstructResult<()> {
        if is_reserved_builtin_name(name) {
            self.fail(reserved_builtin_name(self.file, name_span, name));
            return Ok(());
        }
        if self.lookup(name).is_some() || self.lookup_place(name).is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                name_span,
                format!("`{name}` is already bound in this scope"),
            ));
            return Ok(());
        }
        match self.bind_place_address(name, place_expr) {
            Ok(()) => {}
            Err(LoweringFailure::Recoverable) => {
                // The address's own diagnostic already fired; poison the name so its later
                // uses do not each re-report an unbound place on top of that cause.
                self.poisoned_bindings.insert(name.to_string());
            }
            Err(LoweringFailure::CodeLimitReached) => {
                return Err(LoweringFailure::CodeLimitReached);
            }
        }
        Ok(())
    }

    /// Bind a validated `place` name to its durable entry address, pushing the
    /// [`PlaceLocal`] on success. An unresolved address is a recoverable failure the
    /// resolver has already reported, so the caller can poison the name.
    fn bind_place_address(&mut self, name: &str, place_expr: &Expression) -> ConstructResult<()> {
        let access = match self.durable_access(place_expr) {
            Ok(shape) => shape,
            Err(drift) => {
                self.ledger_drift::<()>(drift);
                return Err(LoweringFailure::Recoverable);
            }
        };
        if !matches!(access, Some(DurShape::Entry)) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                place_expr.span(),
                "a `place` names a whole durable entry address such as `^root(key)`".to_string(),
            ));
            return Err(LoweringFailure::Recoverable);
        }
        let place = self
            .resolve_durable(place_expr)
            .ok_or(LoweringFailure::Recoverable)?;
        let span = place.span;
        let DurTarget::Entry { node, .. } = place.target else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                place_expr.span(),
                "a `place` names a whole durable entry address such as `^root(key)`, not a field"
                    .to_string(),
            ));
            return Err(LoweringFailure::Recoverable);
        };
        // Evaluate each key column of the address exactly once into a fresh slot, root
        // column first, so every later operation through the place reads the slots rather
        // than re-running the key operands. A branch place binds its whole key-path.
        let mut key_slots = Vec::with_capacity(place.keys.len());
        for column in place.keys {
            match column.key {
                PlaceKey::Expr(key_expr) => {
                    let key_slot = self
                        .alloc_slot(key_expr.span())
                        .ok_or(LoweringFailure::Recoverable)?;
                    self.lower_as(key_expr, LTy::bare_scalar(column.key_ty))?;
                    self.push(Instr::LocalSet(key_slot), span)?;
                    key_slots.push((key_slot, column.key_ty));
                }
                PlaceKey::Identity { expr, root, cols } => {
                    // The identity spreads into the addressed root's ordered key columns, so
                    // the place records its whole physical key-path.
                    let columns = self.capture_identity_key_columns(expr, root, cols, span)?;
                    key_slots.extend(columns);
                }
                PlaceKey::Bound(_) => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType,
                        self.file,
                        place_expr.span(),
                        "a `place` names a store address `^root(key)`, not another place"
                            .to_string(),
                    ));
                    return Err(LoweringFailure::Recoverable);
                }
            }
        }
        self.places.push(PlaceLocal {
            name: name.to_string(),
            key_slots,
            node,
        });
        Ok(())
    }

    /// Resolve a durable place, reporting a diagnostic on a bad root name, key arity, or
    /// field name. The returned place holds no borrow of the registry.
    ///
    /// One resolver for both spellings, so a composed operation seals the identical
    /// operation site an inline one does.
    pub(super) fn resolve_durable<'e>(
        &mut self,
        expr: &'e Expression,
    ) -> Option<DurablePlace<'a, 'e>> {
        // A durable access names its store at the `^name` leaf; resolving it here reports
        // a bad name or a parked shape precisely, and a non-address expression is `None`.
        let base = if self.is_place_rooted(expr) {
            EntryBase::Place
        } else {
            EntryBase::Inline(self.resolve_root(saved_root_name(expr)?, expr.span())?)
        };
        match expr {
            // A whole-entry address: a bare place, or `^root[key].b1[k1]….bn[kn]` /
            // `<place>.b1[k1]…` at any depth.
            Expression::Name { span, .. } | Expression::Keyed { span, .. } => {
                let (keys, node) = self.resolve_entry_node(base, expr)?;
                let handle = self.bind_entry_site(node)?;
                Some(DurablePlace {
                    keys,
                    target: DurTarget::Entry { node, handle },
                    family: node.family(),
                    span: *span,
                })
            }
            // A field-exact address `<entry-address>.field`, a whole root-level group
            // `<root-address>.group`, or a group-leaf address `<root-address>.group.leaf`.
            Expression::Field {
                base: entry,
                name: field_name,
                name_span,
                span,
                ..
            } => {
                // Resolved before the entry-address forms because a group leaf's base is a
                // group address, not an entry address.
                if let Some((keys, root, group)) = self.resolve_group_address(base, entry) {
                    let Some((slot, leaf)) = group.field_index(field_name) else {
                        self.report_missing_group_leaf(root, group, field_name, *name_span);
                        return None;
                    };
                    let handle = self.bind_group_site(root, group)?;
                    return Some(DurablePlace {
                        keys,
                        target: DurTarget::GroupLeaf {
                            handle,
                            slot,
                            ty: leaf.ty,
                            required: leaf.required,
                        },
                        family: &root.family,
                        span: *span,
                    });
                }
                let (keys, node) = self.resolve_entry_node(base, entry)?;
                if let Some(field) = node.field(field_name) {
                    let handle = self.bind_field_site(node, &field.path)?;
                    return Some(DurablePlace {
                        keys,
                        target: DurTarget::Field {
                            handle,
                            ty: field.ty,
                            required: field.required,
                        },
                        family: node.family(),
                        span: *span,
                    });
                }
                // A whole root-level group address. Groups are executable only at the root
                // level, so only a root node offers one.
                if let DurNode::Root(root) = node
                    && let Some(group) = root.group(field_name)
                {
                    let handle = self.bind_group_site(root, group)?;
                    return Some(DurablePlace {
                        keys,
                        target: DurTarget::Group {
                            handle,
                            record: group.record,
                            holds_required: group.holds_required(),
                        },
                        family: &root.family,
                        span: *span,
                    });
                }
                self.report_missing_member(&node, field_name, *name_span);
                None
            }
            _ => None,
        }
    }

    /// Resolve a group address `<entry>.group` to its key-path, the addressed root, and
    /// the addressed root-level group, or `None` when `expr` is not one. Only a root node
    /// offers groups, so a field or branch selector resolves cleanly to `None` without a
    /// diagnostic and the caller falls through to the entry-address forms.
    fn resolve_group_address<'e>(
        &mut self,
        base: EntryBase<'a>,
        expr: &'e Expression,
    ) -> Option<(
        Vec<DurKey<'e>>,
        &'a crate::durable::DurableRoot,
        &'a crate::durable::DurableGroup,
    )> {
        let Expression::Field {
            base: entry, name, ..
        } = expr
        else {
            return None;
        };
        if matches!(base, EntryBase::Inline(_)) && !is_entry_address(entry) {
            return None;
        }
        let (keys, node) = self.resolve_entry_node(base, entry)?;
        let DurNode::Root(root) = node else {
            return None;
        };
        let group = root.group(name)?;
        Some((keys, root, group))
    }

    /// Resolve a whole-entry address into its key-path (root-first, one column per hop)
    /// and the addressed node, walking the nested branch chain level by level. Returns
    /// `None` on a shape that is not an entry address, and reports a diagnostic then
    /// `None` on a bad root or branch name.
    ///
    /// The place base stands in for the `^root` leaf, so a branch beneath a place
    /// addresses the same node — and seals the same operation site — an inline
    /// `^root[k].branch[bk]` does.
    pub(super) fn resolve_entry_node<'e>(
        &mut self,
        base: EntryBase<'a>,
        expr: &'e Expression,
    ) -> Option<(Vec<DurKey<'e>>, DurNode<'a>)> {
        match expr {
            // The place base case: an in-scope `place`/pin binding whose key columns were
            // evaluated once, when the binding was taken.
            Expression::Name { segments, .. } => {
                let EntryBase::Place = base else {
                    return None;
                };
                let [name] = &segments[..] else {
                    return None;
                };
                let place = self.lookup_place(name.text())?;
                Some((place.bound_keys(), place.node))
            }
            Expression::Keyed {
                base: head,
                keys,
                span,
                ..
            } => match &**head {
                // The inline base case `^root[k1, …]`: the root whole-entry address, one
                // key operand per root key column in declaration order.
                Expression::SavedRoot {
                    name,
                    span: root_span,
                } => {
                    let EntryBase::Inline(root) = base else {
                        return None;
                    };
                    self.check_root_name(root, name, *root_span)?;
                    // `^root[id]`: one entry-identity operand supplies the whole root key
                    // tuple, spread into the root's key columns at emit. Any entry-identity
                    // operand takes this path; whether it names *this* root is decided by
                    // the identity type check at emit. A per-column key list keeps the
                    // ordinary scalar path.
                    if let [only] = keys.as_slice()
                        && self.identity_operand_root(only).is_some()
                    {
                        let columns = vec![DurKey {
                            // The identity is lowered against its own root type, not a
                            // scalar, so `key_ty` is unused here (emit and capture recover
                            // the per-column scalars from the spread); it carries the first
                            // key column only to satisfy the shared `DurKey` shape.
                            key: PlaceKey::Identity {
                                expr: only,
                                root: root.root_id,
                                cols: root.key.len() as u16,
                            },
                            key_ty: root.key[0],
                        }];
                        return Some((columns, DurNode::Root(root)));
                    }
                    let mut columns = Vec::new();
                    self.push_key_columns(&mut columns, keys, &root.key, *span)?;
                    Some((columns, DurNode::Root(root)))
                }
                // The recursive case `<entry-address>.branch[bk1, …]`: extend the parent
                // entry's key-path with this branch's own key columns in declaration order.
                Expression::Field {
                    base: parent_base,
                    name: branch_name,
                    name_span: branch_span,
                    ..
                } => {
                    let (mut columns, parent) = self.resolve_entry_node(base, parent_base)?;
                    let Some(branch) = parent.branch(branch_name) else {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType,
                            self.file,
                            *branch_span,
                            parent.no_branch_message(branch_name),
                        ));
                        return None;
                    };
                    self.push_key_columns(&mut columns, keys, &branch.key, *span)?;
                    Some((columns, parent.child(branch)))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Match the positional key operands of one node against its ordered key columns,
    /// pushing one [`DurKey`] per column onto `columns` in declaration order. Reports a
    /// diagnostic and returns `None` on a wrong operand count. The keyed-access grammar
    /// already forbids a named key, so only arity is checked here.
    fn push_key_columns<'e>(
        &mut self,
        columns: &mut Vec<DurKey<'e>>,
        keys: &'e [Expression],
        key_columns: &[ScalarType],
        span: SourceSpan,
    ) -> Option<()> {
        if keys.len() != key_columns.len() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                format!(
                    "a store access takes {} positional key column(s), one per key column",
                    key_columns.len()
                ),
            ));
            return None;
        }
        for (key, &key_ty) in keys.iter().zip(key_columns) {
            columns.push(DurKey {
                key: PlaceKey::Expr(key),
                key_ty,
            });
        }
        Some(())
    }

    fn check_root_name(
        &mut self,
        root: &crate::durable::DurableRoot,
        name: &str,
        span: SourceSpan,
    ) -> Option<()> {
        if root.name == name {
            Some(())
        } else {
            self.fail(name_not_in_scope(
                self.file,
                span,
                NameFamily::Root,
                name,
                None,
            ));
            None
        }
    }

    /// The store-root index a key operand names when it is statically an entry identity:
    /// a binding of identity type (`^root[id]`), or an `Id(^root, …)` constructor call.
    /// Non-emitting: it only inspects the binding environment and the call spelling.
    fn identity_operand_root(&self, expr: &Expression) -> Option<RootId> {
        match expr {
            Expression::Name { segments, .. } => match &segments[..] {
                [name] => self
                    .lookup(name.text())
                    .and_then(|local| local.ty.bare_identity()),
                _ => None,
            },
            Expression::Call { callee, .. } => match &**callee {
                Expression::Name { segments, .. } if matches!(&segments[..], [n] if n.text() == "Id") => {
                    Some(RootId::from_index(0))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Lower a durable read (`^r(k)` entry, `^r(k).branch(bk)` branch entry, `^r(k).f`
    /// field, or the place forms).
    pub(super) fn lower_durable_read(
        &mut self,
        place: DurablePlace<'a, '_>,
    ) -> ConstructResult<LTy> {
        let checked_slots = if matches!(
            &place.target,
            DurTarget::Field { required: true, .. } | DurTarget::GroupLeaf { required: true, .. }
        ) {
            self.checked_key_slots(place.family, place.bound_key_path(), place.span)?
        } else {
            None
        };
        if checked_slots.is_none() {
            self.emit_key_path(&place.keys, place.span)?;
        }
        Ok(match place.target {
            DurTarget::Entry { node, handle } => {
                let site = self
                    .site_operand(&handle)
                    .ok_or(LoweringFailure::Recoverable)?;
                self.push(Instr::DurReadEntry(site), place.span)?;
                LTy::Record {
                    ty: node.record(),
                    optional: true,
                }
            }
            DurTarget::Field { handle, ty, .. } => {
                let site = self
                    .site_operand(&handle)
                    .ok_or(LoweringFailure::Recoverable)?;
                if let Some(key_slots) = checked_slots {
                    self.push(Instr::DurReadFieldPresent { site, key_slots }, place.span)?;
                    garg_to_lty(ty)
                } else {
                    self.push(Instr::DurReadField(site), place.span)?;
                    garg_to_lty(ty).to_optional()
                }
            }
            // A whole root-level group materializes as one optional group record: the
            // group's own leaves, present exactly when the entry is present.
            DurTarget::Group { handle, record, .. } => {
                let site = self
                    .site_operand(&handle)
                    .ok_or(LoweringFailure::Recoverable)?;
                self.push(Instr::DurReadGroup(site), place.span)?;
                LTy::Record {
                    ty: record,
                    optional: true,
                }
            }
            // A group leaf materializes the whole group before projecting one slot. A
            // proved required leaf is bare; untested and sparse reads retain the optional
            // result and absent-entry branch.
            DurTarget::GroupLeaf {
                handle,
                slot,
                ty,
                required,
                ..
            } => {
                let site = self
                    .site_operand(&handle)
                    .ok_or(LoweringFailure::Recoverable)?;
                if let Some(key_slots) = checked_slots {
                    self.push(Instr::DurReadGroupPresent { site, key_slots }, place.span)?;
                    self.push(Instr::FieldGet(slot), place.span)?;
                    return Ok(garg_to_lty(ty));
                }
                self.push(Instr::DurReadGroup(site), place.span)?;
                let result = garg_to_lty(ty).to_optional();
                let to_absent = self.push_branch_present(place.span)?;
                self.push(Instr::FieldGet(slot), place.span)?;
                if required {
                    self.push(Instr::SomeWrap, place.span)?;
                }
                let to_end = self.push_jump(place.span)?;
                let absent = self.here();
                self.patch(to_absent, absent);
                self.push(Instr::VacantLoad(result.image()), place.span)?;
                let end = self.here();
                self.patch(to_end, end);
                result
            }
        })
    }

    /// Lower `exists(place)`. A specific entry or field address (`^root(key)`,
    /// `^root(key).field`, a named `place`) is a keyed presence probe; a store root
    /// (`^root`) or a keyed branch family (`^root(key).notes`) instead asks whether that
    /// family has at least one payload-bearing child.
    pub(super) fn lower_exists(
        &mut self,
        args: &[Argument],
        span: SourceSpan,
    ) -> ConstructResult<LTy> {
        let [arg] = args else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                "`exists` takes one store place".to_string(),
            ));
            return Err(LoweringFailure::Recoverable);
        };
        // A unique index is a complete-key probe (the presence half of the `if const`
        // lookup); a nonunique index is scan-only and has no keyed presence probe.
        let index_read = match self.resolve_index_read(&arg.value) {
            Ok(read) => read,
            Err(drift) => {
                self.ledger_drift::<()>(drift);
                return Err(LoweringFailure::Recoverable);
            }
        };
        if let Some(read) = index_read {
            if read.index.unique {
                return self.lower_index_exists(read.root, read.index, read.keys, arg.value.span());
            }
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                arg.value.span(),
                format!(
                    "the non-unique index `{}` is scan-only and has no `exists` probe; scan it \
                     with a `for` head",
                    read.index.name
                ),
            ));
            return Err(LoweringFailure::Recoverable);
        }
        // A family argument names no immediate child key, so it reuses the traversal place
        // resolver and emits only the ancestor key-path. A scalar-field tail is not a
        // family and falls through to the keyed cell probe.
        let is_family = match self.arg_is_family(&arg.value) {
            Ok(family) => family,
            Err(drift) => {
                self.ledger_drift::<()>(drift);
                return Err(LoweringFailure::Recoverable);
            }
        };
        if is_family {
            let target = self
                .resolve_traversal_place(&arg.value)
                .ok_or(LoweringFailure::Recoverable)?;
            let site = self
                .entry_site_operand(target.node)
                .ok_or(LoweringFailure::Recoverable)?;
            self.emit_key_path(&target.ancestor_keys, target.span)?;
            self.push(Instr::DurFamilyExists(site), span)?;
            return Ok(LTy::bare_scalar(ScalarType::Bool));
        }
        // A specific addressed cell (an entry or a field) probes that one cell's presence.
        let access = match self.durable_access(&arg.value) {
            Ok(shape) => shape,
            Err(drift) => {
                self.ledger_drift::<()>(drift);
                return Err(LoweringFailure::Recoverable);
            }
        };
        if access.is_some() {
            let place = self
                .resolve_durable(&arg.value)
                .ok_or(LoweringFailure::Recoverable)?;
            self.emit_key_path(&place.keys, place.span)?;
            let site = match place.target {
                DurTarget::Entry { handle, .. } | DurTarget::Field { handle, .. } => self
                    .site_operand(&handle)
                    .ok_or(LoweringFailure::Recoverable)?,
                // A group is markerless — its presence is the entry's — and a group leaf
                // has no site of its own, so probe the containing entry instead.
                DurTarget::Group { .. } | DurTarget::GroupLeaf { .. } => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckUnsupported,
                        self.file,
                        arg.value.span(),
                        "`exists` over a group or a group leaf is not supported; probe the \
                         containing entry `^root(key)`"
                            .to_string(),
                    ));
                    return Err(LoweringFailure::Recoverable);
                }
            };
            self.push(Instr::DurExists(site), place.span)?;
            return Ok(LTy::bare_scalar(ScalarType::Bool));
        }
        // A poisoned `const`/`place` was already reported at its binding, so its `exists`
        // use adds no diagnostic.
        if let Expression::Name { segments, .. } = &arg.value
            && let [name] = &segments[..]
            && self.poisoned_bindings.contains(name.text())
        {
            self.failed = true;
            return Err(LoweringFailure::Recoverable);
        }
        self.fail(SourceDiagnostic::at(
            Code::CheckType,
            self.file,
            arg.value.span(),
            "`exists` takes a store place such as `^root(key)`, a field, a store root, or a \
             keyed branch family"
                .to_string(),
        ));
        Err(LoweringFailure::Recoverable)
    }

    /// Lower `Id(^root, keys…)`: construct the entry identity of the declared store root
    /// from its explicit key columns, without reading the store. The first argument is the
    /// saved-root reference `^root`; the rest are one value per key column in declaration
    /// order, each checked against that column's scalar type.
    pub(super) fn lower_identity_ctor(
        &mut self,
        args: &[Argument],
        span: SourceSpan,
    ) -> ConstructResult<LTy> {
        if args.iter().any(|arg| arg.name.is_some()) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                "`Id` takes positional arguments: a store root then one value per key column"
                    .to_string(),
            ));
            return Err(LoweringFailure::Recoverable);
        }
        let Some((root_arg, key_args)) = args.split_first() else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                "`Id` takes a store root `^root` then one value per key column".to_string(),
            ));
            return Err(LoweringFailure::Recoverable);
        };
        let Expression::SavedRoot {
            name: root_name,
            span: root_span,
        } = &root_arg.value
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                root_arg.value.span(),
                "`Id`'s first argument is the store root `^root`".to_string(),
            ));
            return Err(LoweringFailure::Recoverable);
        };
        let root = self
            .resolve_root(root_name, *root_span)
            .ok_or(LoweringFailure::Recoverable)?;
        let key_columns = root.key.clone();
        if key_args.len() != key_columns.len() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                format!(
                    "`Id(^{root_name}, …)` takes {} key column value(s), one per key column",
                    key_columns.len()
                ),
            ));
            return Err(LoweringFailure::Recoverable);
        }
        // Push each key column root-first in declaration order, coerced to the column's
        // scalar type, so `MakeIdentity` pops them into the tuple in column order.
        for (arg, &key_ty) in key_args.iter().zip(&key_columns) {
            self.lower_as(&arg.value, LTy::bare_scalar(key_ty))?;
        }
        self.push(
            Instr::MakeIdentity {
                root: root.root_id,
                cols: key_columns.len() as u16,
            },
            span,
        )?;
        Ok(LTy::Identity {
            root: root.root_id,
            optional: false,
        })
    }

    /// Lower a durable assignment: a whole-entry upsert (root or branch), or a field,
    /// group, or group-leaf write on an entry a presence proof covers.
    pub(super) fn lower_durable_assign(
        &mut self,
        place: DurablePlace<'a, '_>,
        value: &Expression,
    ) -> ConstructResult<()> {
        let family = place.family;
        match &place.target {
            DurTarget::Entry { node, handle } => {
                let (handle, record) = (handle.clone(), node.record());
                self.lower_upsert(&place.keys, &handle, record, value, place.span)?;
                // A whole-entry assignment through a place leaves the entry present on
                // every path from here, so the rest of the block may write through it.
                if let Some(key_slots) = place.bound_key_path() {
                    self.mark_present(family, key_slots);
                }
            }
            DurTarget::Field { handle, ty, .. } => {
                let ty = *ty;
                let key_slots = place.bound_key_path();
                let site = self
                    .site_operand(handle)
                    .ok_or(LoweringFailure::Recoverable)?;
                self.lower_definite_field_value(value, garg_to_lty(ty), place.span)?;
                let key_slots = self.require_present(family, key_slots, place.span)?;
                self.push(Instr::DurSetField { site, key_slots }, place.span)?;
            }
            // `p.group = R.group(…)`: an exact whole-group replacement, group-scoped — the
            // entry's other groups, top-level fields, and branches are untouched. The key
            // slots are captured before the RHS; proof use follows its effects.
            DurTarget::Group { handle, record, .. } => {
                let record = *record;
                let key_slots = place.bound_key_path();
                let site = self
                    .site_operand(handle)
                    .ok_or(LoweringFailure::Recoverable)?;
                self.lower_as(
                    value,
                    LTy::Record {
                        ty: record,
                        optional: false,
                    },
                )?;
                let key_slots = self.require_present(family, key_slots, place.span)?;
                self.push(Instr::DurReplaceGroup { site, key_slots }, place.span)?;
            }
            // `p.group.leaf = value`: a whole-group read-modify-write over the proven entry.
            DurTarget::GroupLeaf {
                handle, slot, ty, ..
            } => {
                let (handle, slot, ty) = (handle.clone(), *slot, *ty);
                self.lower_group_leaf_set(
                    (family, place.bound_key_path()),
                    &handle,
                    slot,
                    value,
                    ty,
                    place.span,
                )?;
            }
        }
        Ok(())
    }

    /// Lower the operand of a durable field set: a definite value of the field's bare
    /// type. `absent` and any optional operand are refused naming `delete`, so a field has
    /// one clearing spelling.
    fn lower_definite_field_value(
        &mut self,
        value: &Expression,
        bare: LTy,
        span: SourceSpan,
    ) -> ConstructResult<()> {
        // A built-in or collection constructor is directed by the bare expected type;
        // every other operand is judged on its own type, so an optional operand is refused
        // at the write rather than reported as a mismatch at the operand.
        let optional = match value {
            Expression::Absent { .. } => true,
            _ if constructor_kind(value).is_some() || collection_ctor_call(value).is_some() => {
                return self.lower_as(value, bare);
            }
            _ => {
                let got = self.lower_expr(value)?;
                if !got.is_optional() && got != bare {
                    self.fail(type_mismatch(
                        self.records,
                        self.file,
                        value.span(),
                        got,
                        bare,
                    ));
                    return Err(LoweringFailure::Recoverable);
                }
                got.is_optional()
            }
        };
        if optional {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                format!(
                    "a durable field is set to a definite {} value; to clear it, write \
                     `delete` on the field",
                    bare.spelling(self.records)
                ),
            ));
            return Err(LoweringFailure::Recoverable);
        }
        Ok(())
    }

    /// Lower `delete ^r(k)` / `delete ^r(k).branch(bk)` (entry payload erase) or
    /// `delete ^r(k).f` (sparse-field erase).
    pub(super) fn lower_durable_delete(
        &mut self,
        path: &Expression,
        span: SourceSpan,
    ) -> ConstructResult<()> {
        let access = match self.durable_access(path) {
            Ok(shape) => shape,
            Err(drift) => {
                self.record_invariant(LowerInvariant::from(drift));
                return Ok(());
            }
        };
        if access.is_none() {
            self.fail(unsupported(self.file, span, "this delete target"));
            return Ok(());
        }
        let Some(place) = self.resolve_durable(path) else {
            return Ok(());
        };
        // A group-leaf clear is a whole-group read-modify-write (its key-path is evaluated
        // inside the helper), so it is handled before the shared single key-path emission.
        if let DurTarget::GroupLeaf {
            handle,
            slot,
            required,
            ..
        } = &place.target
        {
            if *required {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType,
                    self.file,
                    place.span,
                    "a required group leaf cannot be deleted".to_string(),
                ));
                return Ok(());
            }
            let (handle, slot, span) = (handle.clone(), *slot, place.span);
            self.lower_group_leaf_unset(&place.keys, &handle, slot, span)?;
            return Ok(());
        }
        let family = place.family;
        // A group holding a required leaf is part of every present entry; it is erased
        // only with its entry. Refused before any key operand is evaluated.
        if let DurTarget::Group {
            holds_required: true,
            ..
        } = &place.target
        {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                place.span,
                "a group with a required leaf is erased only with its entry".to_string(),
            ));
            return Ok(());
        }
        self.emit_key_path(&place.keys, place.span)?;
        match place.target {
            DurTarget::Entry { handle, .. } => {
                let site = self
                    .site_operand(&handle)
                    .ok_or(LoweringFailure::Recoverable)?;
                self.push(Instr::DurEraseEntry(site), place.span)?;
                // The erased entry may be any entry of the family a proof covers, so every
                // proof over the family ends here.
                self.erase_family(family);
            }
            DurTarget::Field {
                handle, required, ..
            } => {
                if required {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType,
                        self.file,
                        place.span,
                        "a required field cannot be deleted".to_string(),
                    ));
                    return Ok(());
                }
                let site = self
                    .site_operand(&handle)
                    .ok_or(LoweringFailure::Recoverable)?;
                self.push(Instr::DurEraseField(site), place.span)?;
            }
            // `delete p.group` over all-sparse leaves: erase only that group's leaves; the
            // entry's other groups, top-level fields, and branches are untouched.
            DurTarget::Group { handle, .. } => {
                let site = self
                    .site_operand(&handle)
                    .ok_or(LoweringFailure::Recoverable)?;
                self.push(Instr::DurEraseGroup(site), place.span)?;
            }
            #[expect(
                clippy::unreachable,
                reason = "lowering bookkeeping: a group-leaf delete is dispatched on a dedicated path before this shared key-path emit, so it never reaches this arm"
            )]
            DurTarget::GroupLeaf { .. } => {
                unreachable!("a group-leaf delete is handled before the shared key-path emit")
            }
        }
        Ok(())
    }
}
