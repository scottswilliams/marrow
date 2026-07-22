//! The durable-place model and the lowering of durable reads, writes, presence, traversal, and managed-index access.

use super::*;

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
    /// A key already evaluated once into a local slot (a named `place`); each use
    /// reads the slot with `LocalGet`, so the operand runs exactly once at the
    /// binding no matter how many operations flow through the place.
    Bound(u16),
    /// The whole root key-path supplied by one entry-identity operand (`^root[id]`):
    /// the identity is lowered against the addressed root's identity type (`root`), then
    /// `IdentityKeyPath` spreads it into the root's `cols` key columns. One `Identity` key
    /// stands for every root key column, so it is only the root whole-key form and never
    /// mixes with per-column keys. `root` is the addressed root's RootId, so an identity
    /// minted over a different root is a type mismatch here.
    Identity {
        expr: &'e Expression,
        root: u16,
        cols: u16,
    },
}

/// One column of a durable operation's key-path: how it reaches the stack and its
/// scalar type. A single-key root entry is a one-column path `[root_key]`; a
/// single-level branch entry is a two-column path `[root_key, branch_key]`, pushed
/// root-first so the innermost key is on top (the order the kernel's `pop_key_path`
/// expects). A composite-key root has several root key columns rather than one.
#[derive(Clone, Copy)]
pub(super) struct DurKey<'e> {
    pub(super) key: PlaceKey<'e>,
    pub(super) key_ty: ScalarType,
}

/// A resolved durable place: the key-path that addresses its node and its target. The
/// path columns are inline operand expressions or a source-local `place`'s
/// pre-evaluated slots; the target is the whole entry or one field.
pub(super) struct DurablePlace<'e> {
    keys: Vec<DurKey<'e>>,
    target: DurTarget,
    span: SourceSpan,
}

impl DurablePlace<'_> {
    /// The single root key slot when this place's whole key-path is one pre-evaluated
    /// `Bound` column. `None` for an inline key or any multi-column key path — a branch
    /// or a composite-key root. Used only by the whole-entry root upsert, which
    /// establishes root presence for that one slot.
    fn root_bound_slot(&self) -> Option<u16> {
        match self.keys.as_slice() {
            [
                DurKey {
                    key: PlaceKey::Bound(slot),
                    ..
                },
            ] => Some(*slot),
            _ => None,
        }
    }

    /// This place's whole key-path as pre-evaluated slots (root-first) when *every*
    /// column is a `Bound` slot — the shape a strict present-entry field set and a
    /// place-entry presence guard require, for a root or a branch place. `None` if any
    /// column is an inline key expression (the strict form needs pre-evaluated slots).
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

/// Whether a resolved durable entry addresses a store root or a keyed `branch` — its
/// durable node kind. A named `place` records this at its binding from the canonical
/// resolved durable node and resolves its fields by it: a root against its entry site, a
/// branch against its materialized record. The kind is independent of the key-operand
/// count — a composite-key root carries several key operands yet is still a root.
#[derive(Clone, Copy)]
pub(super) enum PlaceNodeKind {
    Root,
    Branch,
}

/// A source-local named `place`: a durable entry designation whose key columns were
/// evaluated exactly once into `key_slots` at the binding. Whole-entry and field
/// operations through the place read those slots rather than re-evaluating the key
/// operands. `node_kind` records whether the place addresses a store root or a keyed
/// branch — taken from the canonical resolved durable node — and drives field resolution
/// independently of how many key slots the place carries.
pub(super) struct PlaceLocal {
    pub(super) name: String,
    pub(super) key_slots: Vec<(u16, ScalarType)>,
    pub(super) entry_site: u16,
    pub(super) record: TypeId,
    pub(super) node_kind: PlaceNodeKind,
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

impl PlaceLocal {
    /// This place's whole key-path as pre-evaluated slots (root-first) — the key-path a
    /// strict present-entry field set reads and a presence guard proves, for a root or a
    /// branch place uniformly.
    fn key_path_slots(&self) -> Vec<u16> {
        self.key_slots.iter().map(|(slot, _)| *slot).collect()
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

/// A resolved durable target: the whole entry, one field, a whole root-level group, or one
/// group leaf.
#[derive(Clone, Copy)]
enum DurTarget {
    Entry {
        entry_site: u16,
        record: TypeId,
        /// The node kind of the addressed entry — a store root or a keyed branch. A `place`
        /// binding records it so its later field access resolves against the right owner
        /// independently of the key-operand count; a whole-entry read/write/erase ignores it.
        node_kind: PlaceNodeKind,
    },
    Field {
        site: u16,
        /// The field's value type (a scalar or a widened composite), from which the
        /// read result and written-value type are built.
        ty: GArg,
        required: bool,
    },
    /// A whole root-level `group` (`^root(k).group`): read, replaced, or erased as one
    /// materialized `record` value through the `GroupEntry` site `entry_site`.
    Group { entry_site: u16, record: TypeId },
    /// One leaf of a root-level group (`^root(k).group.leaf`). A read materializes the
    /// whole group through `entry_site` and projects `slot`; a write or clear is a
    /// whole-group read-modify-write that rewrites `slot` on the read-back group record and
    /// replaces the group, so a leaf never has a durable site of its own.
    GroupLeaf {
        entry_site: u16,
        slot: u16,
        ty: GArg,
        required: bool,
    },
}

/// The leaf edit a group-leaf read-modify-write applies to the materialized group record:
/// set the leaf present to a bare value, or clear a sparse leaf to vacant.
enum GroupLeafEdit<'e> {
    Set { value: &'e Expression, ty: GArg },
    Unset,
}

/// A node reached along a resolved durable entry address: the root, or a keyed branch on
/// the address's branch chain. Both expose the same navigation — a nested branch by name,
/// a stored field, a whole-entry site, and a materialized record — so the recursive address
/// resolver walks them uniformly at any depth.
#[derive(Clone, Copy)]
pub(super) enum DurNode<'a> {
    Root(&'a crate::durable::DurableRoot),
    Branch(&'a crate::durable::DurableBranch),
}

/// The pieces of one resolved durable field a [`DurTarget::Field`] needs, projected from a
/// root field or a branch field uniformly.
struct DurFieldRef {
    /// The field's stable field-leaf path. The caller allocates (and deduplicates) its
    /// operation site through the draft when it builds the field target, so an untouched
    /// field mints no site.
    path: SemanticPath,
    /// The field's value type: a root field's widened value set, or a branch field's
    /// scalar (branch fields are currently scalar-only) lifted to `GArg::Scalar`.
    ty: GArg,
    required: bool,
}

impl<'a> DurNode<'a> {
    /// This node's durable kind, recorded on a `place` that binds it so later field access
    /// resolves against the right owner without re-inspecting the address.
    fn place_node_kind(&self) -> PlaceNodeKind {
        match self {
            DurNode::Root(_) => PlaceNodeKind::Root,
            DurNode::Branch(_) => PlaceNodeKind::Branch,
        }
    }

    fn entry_site(&self) -> u16 {
        match self {
            DurNode::Root(root) => root.entry_site,
            DurNode::Branch(branch) => branch.entry_site,
        }
    }

    fn record(&self) -> TypeId {
        match self {
            DurNode::Root(root) => root.record,
            DurNode::Branch(branch) => branch.record,
        }
    }

    pub(super) fn branch(&self, name: &str) -> Option<&'a crate::durable::DurableBranch> {
        match self {
            DurNode::Root(root) => root.branch(name),
            DurNode::Branch(branch) => branch.branch(name),
        }
    }

    fn field(&self, name: &str) -> Option<DurFieldRef> {
        match self {
            DurNode::Root(root) => root.field(name).map(|field| DurFieldRef {
                path: field.path.clone(),
                ty: field.ty,
                required: field.required,
            }),
            DurNode::Branch(branch) => branch.field(name).map(|field| DurFieldRef {
                path: field.path.clone(),
                ty: GArg::Scalar(field.scalar),
                required: field.required,
            }),
        }
    }

    fn name(&self) -> &str {
        match self {
            DurNode::Root(root) => &root.name,
            DurNode::Branch(branch) => &branch.name,
        }
    }

    fn no_field_message(&self, field: &str) -> String {
        match self {
            DurNode::Root(root) => format!("`{}` has no field `{field}`", root.name),
            DurNode::Branch(branch) => {
                format!("branch `{}` has no field `{field}`", branch.name)
            }
        }
    }

    pub(super) fn no_branch_message(&self, branch: &str) -> String {
        format!("`{}` has no keyed branch `{branch}`", self.name())
    }
}

/// A resolved durable traversal place: the traversed layer's whole-entry site, the
/// immediate key type it enumerates, and the ancestor key-path locating its parent
/// entry (empty for a root family, `[root_key]` for a single-level branch family). The
/// bounded traversal opcode pushes the ancestor path root-first, then the optional
/// inclusive `from` key, and freezes the traversed layer's immediate keys.
pub(super) struct TraversalTarget<'e> {
    pub(super) entry_site: u16,
    pub(super) key_ty: ScalarType,
    /// The materialized record of the traversed family's entry — the shape a two-binding
    /// traversal's per-iteration address pin (`for k, p in …`) binds `p` over.
    pub(super) record: TypeId,
    /// The node kind of the traversed layer — a store root or a keyed branch — carried onto
    /// the per-iteration address pin so its field access resolves by node kind, not by the
    /// ancestor-plus-key slot count.
    pub(super) node_kind: PlaceNodeKind,
    pub(super) ancestor_keys: Vec<DurKey<'e>>,
    pub(super) span: SourceSpan,
}

/// Whether an instruction is a direct durable-place operation — a read, write,
/// presence probe, erase, or managed-index access over a `^` place. A `Duration*`
/// arithmetic opcode is not one. The test-body strict-separation check uses this to
/// tell a body that touches durable data directly from one that only drives exports.
pub(crate) fn is_durable_place_op(instr: &Instr) -> bool {
    matches!(
        instr,
        Instr::DurExists(_)
            | Instr::DurFamilyExists(_)
            | Instr::DurReadField(_)
            | Instr::DurReadEntry(_)
            | Instr::DurReadGroup(_)
            | Instr::DurSetRequired(_)
            | Instr::DurSetSparse(_)
            | Instr::DurSetSparsePresent { .. }
            | Instr::DurCreateEntry(_)
            | Instr::DurReplaceEntry(_)
            | Instr::DurReplaceGroup(_)
            | Instr::DurEraseField(_)
            | Instr::DurEraseEntry(_)
            | Instr::DurEraseGroup(_)
            | Instr::DurIterateBounded { .. }
            | Instr::DurIndexScan { .. }
            | Instr::DurIndexLookup(_)
            | Instr::DurIndexExists(_)
    )
}

/// Whether an instruction stages a durable mutation (a write, replacement, or
/// erase). The requires-ambient-transaction check treats these as the sites that
/// demand a transaction; it mirrors the verifier's mutation classification over the
/// same opcode set. The match is exhaustive over `Instr` — the closed complement is
/// listed rather than elided — so a new opcode fails to compile until it is
/// classified here, welding this owner to the instruction set.
pub(crate) fn is_mutation_instr(instr: &Instr) -> bool {
    match instr {
        Instr::DurSetRequired(_)
        | Instr::DurSetSparse(_)
        | Instr::DurSetSparsePresent { .. }
        | Instr::DurCreateEntry(_)
        | Instr::DurReplaceEntry(_)
        | Instr::DurReplaceGroup(_)
        | Instr::DurEraseField(_)
        | Instr::DurEraseEntry(_)
        | Instr::DurEraseGroup(_) => true,
        Instr::ConstLoad(_)
        | Instr::LocalGet(_)
        | Instr::LocalSet(_)
        | Instr::Pop
        | Instr::Return
        | Instr::Call(_)
        | Instr::Jump(_)
        | Instr::JumpIfFalse(_)
        | Instr::BranchPresent(_)
        | Instr::Unreachable(_)
        | Instr::Todo(_)
        | Instr::Assert
        | Instr::IntAdd
        | Instr::IntSub
        | Instr::IntMul
        | Instr::IntRem
        | Instr::IntDiv
        | Instr::IntNeg
        | Instr::BoolNot
        | Instr::IntLt
        | Instr::IntLe
        | Instr::IntGt
        | Instr::IntGe
        | Instr::EqInt
        | Instr::EqBool
        | Instr::EqText
        | Instr::TextConcat
        | Instr::TextLt
        | Instr::TextLe
        | Instr::TextGt
        | Instr::TextGe
        | Instr::EqBytes
        | Instr::BytesLt
        | Instr::BytesLe
        | Instr::BytesGt
        | Instr::BytesGe
        | Instr::ConvString
        | Instr::ConvBytesText
        | Instr::TextIsEmpty
        | Instr::TextContains
        | Instr::TextTrim
        | Instr::TextSplit(_)
        | Instr::TextLines(_)
        | Instr::TextJoin
        | Instr::EqDate
        | Instr::DateLt
        | Instr::DateLe
        | Instr::DateGt
        | Instr::DateGe
        | Instr::EqInstant
        | Instr::InstantLt
        | Instr::InstantLe
        | Instr::InstantGt
        | Instr::InstantGe
        | Instr::EqDuration
        | Instr::DurationLt
        | Instr::DurationLe
        | Instr::DurationGt
        | Instr::DurationGe
        | Instr::DateAddDays
        | Instr::DateDaysBetween
        | Instr::DurationAdd
        | Instr::DurationSub
        | Instr::InstantAddDuration
        | Instr::InstantSubDuration
        | Instr::IntAddChecked(_)
        | Instr::IntSubChecked(_)
        | Instr::IntMulChecked(_)
        | Instr::IntNegChecked(_)
        | Instr::IntDivChecked(_)
        | Instr::IntRemChecked(_)
        | Instr::RangeGuard { .. }
        | Instr::RecordNew(_)
        | Instr::FieldGet(_)
        | Instr::FieldSet(_)
        | Instr::FieldUnset(_)
        | Instr::SomeWrap
        | Instr::VacantLoad(_)
        | Instr::EnumConstruct { .. }
        | Instr::EnumTag
        | Instr::EnumPayloadGet { .. }
        | Instr::EqEnum
        | Instr::EqId
        | Instr::MakeIdentity { .. }
        | Instr::IdentityKeyPath(_)
        | Instr::DurExists(_)
        | Instr::DurFamilyExists(_)
        | Instr::DurReadField(_)
        | Instr::DurReadEntry(_)
        | Instr::DurReadGroup(_)
        | Instr::DurIterateBounded { .. }
        | Instr::TxnBegin
        | Instr::TxnCommit
        | Instr::DurIndexScan { .. }
        | Instr::DurIndexLookup(_)
        | Instr::DurIndexExists(_)
        | Instr::ListNew(_)
        | Instr::ListAppend
        | Instr::ListLen
        | Instr::ListGet
        | Instr::ListIndex
        | Instr::MapNew(_)
        | Instr::MapInsert
        | Instr::MapRemove
        | Instr::MapGet
        | Instr::MapLen
        | Instr::MapKeyAt
        | Instr::MapValueAt => false,
    }
}

impl<'a> FnLowerer<'a> {
    // --- durable places (design §D) ---

    /// Detect the inline durable shape of a place expression: a whole-entry address
    /// `^root(key)….b(bkey)` at any depth, or a field-exact address `<entry-address>.field`.
    /// No diagnostics. Does not see source-local `place` bindings, which need instance
    /// state; use [`Self::durable_access`] for the full detection.
    pub(super) fn durable_shape(expr: &Expression) -> Option<DurShape> {
        if is_entry_address(expr) {
            Some(DurShape::Entry)
        } else if is_field_address(expr) || is_group_leaf_address(expr) {
            // A field-exact address, a whole root-level group (both `<entry>.name`), or a
            // group-leaf address `<entry>.group.leaf`. The resolver disambiguates a group
            // from a field by name; a group leaf is one field selection deeper.
            Some(DurShape::Field)
        } else {
            None
        }
    }

    /// The inline durable ^-address shape of `expr`, confirming a group-leaf address
    /// against the resolved durable model. [`Self::durable_shape`] recognizes a group-leaf
    /// address `<entry>.mid.leaf` syntactically; here `mid` must actually name a root-level
    /// group. A `mid` that is a stored field (or an unknown name) leaves the expression an
    /// ordinary field projection on a durable field value, lowered and diagnosed by the
    /// ordinary field path rather than compiling to a codeless durable body.
    pub(super) fn durable_shape_here(&self, expr: &Expression) -> Option<DurShape> {
        if is_group_leaf_address(expr) {
            return self.middle_names_a_group(expr).then_some(DurShape::Field);
        }
        Self::durable_shape(expr)
    }

    /// Whether the middle selector of a group-leaf address `<entry>.mid.leaf` names a
    /// root-level `group`: the entry is the root itself (`^root[k]`, not a nested branch,
    /// which offers no executable group) and the root declares a group named `mid`.
    fn middle_names_a_group(&self, expr: &Expression) -> bool {
        let Expression::Field { base, .. } = expr else {
            return false;
        };
        let Expression::Field {
            base: entry,
            name: mid,
            ..
        } = base.as_ref()
        else {
            return false;
        };
        let Expression::Keyed {
            base: root_base, ..
        } = entry.as_ref()
        else {
            return false;
        };
        let Expression::SavedRoot { name, .. } = root_base.as_ref() else {
            return false;
        };
        self.durable
            .root_by_name(name)
            .is_some_and(|root| root.group(mid).is_some())
    }

    /// The most recent in-scope `place` binding named `name`, if any.
    pub(super) fn lookup_place(&self, name: &str) -> Option<&PlaceLocal> {
        self.places.iter().rev().find(|place| place.name == name)
    }

    /// The durable node a `place` addresses — its owning root for a root place, its branch
    /// record's branch for a branch place — resolved by the place's recorded node kind. The
    /// one owner that projects a `place` to its [`DurNode`], so field and branch resolution
    /// through a place share the same navigation (`field`/`branch`, `no_field_message`/
    /// `no_branch_message`). `None` only on a registry-inconsistent place. The node borrows
    /// the registry (`'a`), not `&self`, so a diagnostic may still borrow `self` mutably.
    pub(super) fn place_node(&self, place: &PlaceLocal) -> Option<DurNode<'a>> {
        let durable: &'a DurableRegistry = self.durable;
        match place.node_kind {
            PlaceNodeKind::Root => durable
                .root_by_entry_site(place.entry_site)
                .map(DurNode::Root),
            PlaceNodeKind::Branch => durable.branch_by_record(place.record).map(DurNode::Branch),
        }
    }

    /// Record that the entry of the `place` addressed by `key_path` (its whole key-path
    /// as pre-evaluated slots, root-first) is known present from here (a dominating guard
    /// or a completed upsert). Idempotent.
    pub(super) fn mark_present(&mut self, key_path: Vec<u16>) {
        if !self.present_places.contains(&key_path) {
            self.present_places.push(key_path);
        }
    }

    /// Whether a presence fact currently dominates the entry addressed by `key_path`.
    fn is_present_path(&self, key_path: &[u16]) -> bool {
        self.present_places.iter().any(|path| path == key_path)
    }

    /// Drop the presence fact on the entry addressed by `key_path` (its entry may no
    /// longer be present, e.g. after `delete p`).
    fn clear_present_path(&mut self, key_path: &[u16]) {
        self.present_places.retain(|path| path != key_path);
    }

    /// If `cond` is `exists(p)` over an in-scope named `place`, that place's whole
    /// key-path slots (root-first). The guarded (then) block may set the place's sparse
    /// fields in the strict form. Both root and branch places carry a strict-set
    /// presence consumer — the key-path form addresses either uniformly.
    pub(super) fn exists_guard_path(&self, cond: &Expression) -> Option<Vec<u16>> {
        let Expression::Call { callee, args, .. } = cond else {
            return None;
        };
        let Expression::Name { segments, .. } = &**callee else {
            return None;
        };
        if segments.as_slice() != ["exists"] {
            return None;
        }
        let [arg] = args.as_slice() else {
            return None;
        };
        if arg.name.is_some() {
            return None;
        }
        let Expression::Name { segments, .. } = &arg.value else {
            return None;
        };
        let [name] = segments.as_slice() else {
            return None;
        };
        self.lookup_place(name).map(PlaceLocal::key_path_slots)
    }

    /// Whether `name` names an in-scope `place`.
    pub(super) fn is_place_name(&self, expr: &Expression) -> bool {
        matches!(
            expr,
            Expression::Name { segments, .. }
                if matches!(segments.as_slice(), [name] if self.lookup_place(name).is_some())
        )
    }

    /// The durable shape of a place expression, extending [`Self::durable_shape`]
    /// with source-local `place` bindings: a bare place name is a whole-entry
    /// address, and a field access on a place name is a field address.
    /// Resolve a source managed-index read `^root.index[keys]` to its index and the
    /// bracket key operands, or `None` when the expression is not an index read (a
    /// `Keyed` whose base is a field of the store root naming a declared index). The
    /// index reference lives as long as the durable registry, so it may be held across a
    /// mutable lowering call.
    pub(super) fn resolve_index_read<'e>(&self, expr: &'e Expression) -> Option<IndexRead<'a, 'e>> {
        let Expression::Keyed { base, keys, .. } = expr else {
            return None;
        };
        let Expression::Field {
            base: field_base,
            name,
            ..
        } = base.as_ref()
        else {
            return None;
        };
        let Expression::SavedRoot {
            name: root_name, ..
        } = field_base.as_ref()
        else {
            return None;
        };
        let durable: &'a DurableRegistry = self.durable;
        let root = durable.root_by_name(root_name)?;
        let index = root.index(name)?;
        Some(IndexRead {
            index,
            root,
            keys: keys.as_slice(),
        })
    }

    /// Lower a unique index's exact lookup `^root.index[keys]`: check the operands against
    /// the whole projection, then emit `DurIndexLookup`. The result is the optional source
    /// identity `Id(^root)?` — present with the matching entry's identity, or absent — which
    /// an `if const` head unwraps to a bare `Id(^root)`.
    pub(super) fn lower_index_lookup(
        &mut self,
        index: &crate::durable::DurableIndex,
        root_id: u16,
        keys: &[Expression],
        span: SourceSpan,
    ) -> Option<LTy> {
        if keys.len() != index.projection.len() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "unique index `{}` is looked up by its whole projection of {} key(s)",
                    index.name,
                    index.projection.len()
                ),
            ));
            return None;
        }
        let site = index.site;
        // The projection scalar types are copied out first so the operand lowering (a
        // mutable borrow of `self`) does not overlap the index borrow.
        let projection: Vec<ScalarType> = index.projection.clone();
        for (key, key_ty) in keys.iter().zip(&projection) {
            self.lower_as(key, LTy::bare_scalar(*key_ty))?;
        }
        self.push(Instr::DurIndexLookup(site), span);
        Some(LTy::Identity {
            root: root_id,
            optional: true,
        })
    }

    /// Lower a unique index's presence probe `exists(^root.index[keys])`: check the operands
    /// against the whole projection, then emit `DurIndexExists` over the same lookup site.
    /// The result is a bare `bool` — the presence half of [`lower_index_lookup`], without
    /// materializing the found identity.
    pub(super) fn lower_index_exists(
        &mut self,
        index: &crate::durable::DurableIndex,
        keys: &[Expression],
        span: SourceSpan,
    ) -> Option<LTy> {
        if keys.len() != index.projection.len() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "unique index `{}` is probed by its whole projection of {} key(s)",
                    index.name,
                    index.projection.len()
                ),
            ));
            return None;
        }
        let site = index.site;
        // The projection scalar types are copied out first so the operand lowering (a
        // mutable borrow of `self`) does not overlap the index borrow.
        let projection: Vec<ScalarType> = index.projection.clone();
        for (key, key_ty) in keys.iter().zip(&projection) {
            self.lower_as(key, LTy::bare_scalar(*key_ty))?;
        }
        self.push(Instr::DurIndexExists(site), span);
        Some(LTy::bare_scalar(ScalarType::Bool))
    }

    pub(super) fn durable_access(&self, expr: &Expression) -> Option<DurShape> {
        if let Some(shape) = self.durable_shape_here(expr) {
            return Some(shape);
        }
        // A place-rooted composed address extends a named `place`/pin with the same field,
        // group, and branch selectors an inline `^root` address takes. Classification is
        // non-emitting and mirrors the inline `durable_shape_here` split from
        // `resolve_durable`: a whole entry (a bare place, or a place extended by branch
        // hops), or a field cell (a stored field, a whole root-level group, a group leaf,
        // or a branch field). A projection on a durable field *value* (`p.field.sub`) is not
        // a durable cell and falls through to ordinary projection, exactly as the inline
        // form does.
        match expr {
            Expression::Name { .. } => self.is_place_name(expr).then_some(DurShape::Entry),
            // A place-rooted keyed selection `<place>(.branch[bk])+` is a branch entry. It is
            // classified by place-rootedness, not by resolving the branch, so an unknown
            // branch reaches the resolver and reports "no keyed branch" — the same message
            // the inline `^root(k).branch[bk]` form gives — rather than falling through.
            Expression::Keyed { .. } => self.is_place_rooted(expr).then_some(DurShape::Entry),
            // A field cell off a place: a stored field or a whole root-level group off a
            // place-rooted entry base (a bare place, or a branch-hop chain), or a group leaf.
            // The entry-base case is classified syntactically — by place-rootedness of an
            // entry-shaped base — not by resolving it, so an unknown branch in the base
            // reaches the resolver and reports "no keyed branch" rather than falling through
            // to a confusing projection error. A group leaf is confirmed against the model so
            // a projection on a durable field value (`p.field.sub`) still falls through.
            Expression::Field { base, .. } => (self.place_middle_names_a_group(base)
                || (matches!(&**base, Expression::Name { .. } | Expression::Keyed { .. })
                    && self.is_place_rooted(base)))
            .then_some(DurShape::Field),
            _ => None,
        }
    }

    /// Whether the leftmost base of a durable path expression is an in-scope named
    /// `place`/pin — a bare place name, or a place extended by `.field`, `.group[.leaf]`,
    /// or `.branch[bk]` hops. Routes such an expression to [`Self::resolve_place_composed`],
    /// which sources the place's pre-evaluated key columns as the address prefix.
    fn is_place_rooted(&self, expr: &Expression) -> bool {
        match expr {
            Expression::Name { .. } => self.is_place_name(expr),
            Expression::Field { base, .. } | Expression::Keyed { base, .. } => {
                self.is_place_rooted(base)
            }
            _ => false,
        }
    }

    /// The durable node a place-rooted whole-entry address reaches — a bare place name, or a
    /// place extended by `.branch[bk]` hops — navigating the place node and each branch hop
    /// against the registry without emitting. `None` when `expr` is not a place-rooted entry
    /// address or a branch name does not resolve. The non-emitting twin of
    /// [`Self::resolve_place_entry_node`] (as `entry_address_node` is for the inline walker),
    /// borrowing the registry (`'a`), not `&self`.
    fn place_entry_target(&self, expr: &Expression) -> Option<DurNode<'a>> {
        match expr {
            Expression::Name { segments, .. } => {
                let [name] = segments.as_slice() else {
                    return None;
                };
                self.place_node(self.lookup_place(name)?)
            }
            Expression::Keyed { base, .. } => {
                let Expression::Field {
                    base: parent, name, ..
                } = &**base
                else {
                    return None;
                };
                self.place_entry_target(parent)?
                    .branch(name)
                    .map(DurNode::Branch)
            }
            _ => None,
        }
    }

    /// Whether `expr` is the group address `<place-entry>.group` of a place-rooted group
    /// leaf — the place twin of [`Self::middle_names_a_group`]. Non-emitting: it distinguishes
    /// a group leaf `p.group.leaf` (a durable cell) from a projection on a durable struct
    /// field value `p.field.sub` (ordinary projection), so the classifier routes each the way
    /// the inline forms are routed.
    fn place_middle_names_a_group(&self, expr: &Expression) -> bool {
        let Expression::Field { base, name, .. } = expr else {
            return false;
        };
        matches!(self.place_entry_target(base), Some(DurNode::Root(root)) if root.group(name).is_some())
    }

    /// Resolve a place-rooted whole-entry address — a bare place name, or a place extended by
    /// one or more `.branch[bk]` hops — into its key-path (the place's pre-evaluated bound
    /// columns, then each branch hop's key columns) and the addressed node. `None` when
    /// `expr` is not a place-rooted entry address; reports a diagnostic and `None` on an
    /// unknown branch or a wrong branch-key arity. The place base stands in for the `^root`
    /// leaf of [`Self::resolve_entry_address`], so a branch beneath a place addresses the
    /// same node — and seals the same operation site — an inline `^root(k).branch(bk)` does.
    fn resolve_place_entry_node<'e>(
        &mut self,
        expr: &'e Expression,
    ) -> Option<(Vec<DurKey<'e>>, DurNode<'a>)> {
        match expr {
            Expression::Name { segments, .. } => {
                let [name] = segments.as_slice() else {
                    return None;
                };
                let place = self.lookup_place(name)?;
                let keys = place.bound_keys();
                let node = self.place_node(place)?;
                Some((keys, node))
            }
            Expression::Keyed {
                base, keys, span, ..
            } => {
                let Expression::Field {
                    base: parent_base,
                    name: branch_name,
                    name_span: branch_span,
                    ..
                } = &**base
                else {
                    return None;
                };
                let (mut columns, parent) = self.resolve_place_entry_node(parent_base)?;
                let Some(branch) = parent.branch(branch_name) else {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        *branch_span,
                        parent.no_branch_message(branch_name),
                    ));
                    return None;
                };
                self.push_key_columns(&mut columns, keys, &branch.key, *span)?;
                Some((columns, DurNode::Branch(branch)))
            }
            _ => None,
        }
    }

    /// Resolve a place-rooted group address `<place-entry>.group` to its key-path and the
    /// addressed root-level group, or `None` when `expr` is not one. Only a root node offers
    /// groups, so a branch-rooted place or a non-group tail resolves cleanly to `None`
    /// without a diagnostic — the caller falls through to the entry-field forms. The place
    /// twin of [`Self::resolve_group_address`].
    fn resolve_place_group_address<'e>(
        &mut self,
        expr: &'e Expression,
    ) -> Option<(Vec<DurKey<'e>>, &'a crate::durable::DurableGroup)> {
        let Expression::Field { base, name, .. } = expr else {
            return None;
        };
        let (keys, node) = self.resolve_place_entry_node(base)?;
        let DurNode::Root(root) = node else {
            return None;
        };
        let group = root.group(name)?;
        Some((keys, group))
    }

    /// Resolve a place-rooted durable access — a bare place name/pin, or a place extended by
    /// field, group, group-leaf, or branch selectors — into its pre-evaluated address. The
    /// place twin of the inline `^root…` resolution in [`Self::resolve_durable`]: the place's
    /// once-evaluated key columns are the address prefix, and each selector resolves against
    /// the place's node exactly as the inline forms resolve against the store root, so a
    /// composed operation seals the identical operation site. A missing field or branch is a
    /// precise diagnostic.
    fn resolve_place_composed<'e>(&mut self, expr: &'e Expression) -> Option<DurablePlace<'e>> {
        match expr {
            // A bare place name, or a place extended by branch hops: a whole entry.
            Expression::Name { span, .. } | Expression::Keyed { span, .. } => {
                let (keys, node) = self.resolve_place_entry_node(expr)?;
                Some(DurablePlace {
                    keys,
                    target: DurTarget::Entry {
                        entry_site: node.entry_site(),
                        record: node.record(),
                        node_kind: node.place_node_kind(),
                    },
                    span: *span,
                })
            }
            // A field-exact address, a whole root-level group, or a group-leaf address, each
            // rooted at a place.
            Expression::Field {
                base,
                name: field_name,
                name_span,
                span,
                ..
            } => {
                // A group-leaf address: the base resolves to a root-level group on the place
                // root, and this selector names one of its leaves. Resolved before the
                // entry-address forms because its base is a group address, not an entry.
                if let Some((keys, group)) = self.resolve_place_group_address(base) {
                    let Some((slot, leaf)) = group.field_index(field_name) else {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType.as_str(),
                            self.file,
                            *name_span,
                            format!("group `{}` has no field `{field_name}`", group.name),
                        ));
                        return None;
                    };
                    return Some(DurablePlace {
                        keys,
                        target: DurTarget::GroupLeaf {
                            entry_site: group.entry_site,
                            slot,
                            ty: leaf.ty,
                            required: leaf.required,
                        },
                        span: *span,
                    });
                }
                let (keys, node) = self.resolve_place_entry_node(base)?;
                if let Some(field) = node.field(field_name) {
                    let site = self
                        .draft
                        .alloc_site(SiteDef::field_leaf(field.path))
                        .index();
                    return Some(DurablePlace {
                        keys,
                        target: DurTarget::Field {
                            site,
                            ty: field.ty,
                            required: field.required,
                        },
                        span: *span,
                    });
                }
                // A whole root-level group address `<place-entry>.group`. Groups are
                // executable only at the root level, so only a root node offers one.
                if let DurNode::Root(root) = node
                    && let Some(group) = root.group(field_name)
                {
                    return Some(DurablePlace {
                        keys,
                        target: DurTarget::Group {
                            entry_site: group.entry_site,
                            record: group.record,
                        },
                        span: *span,
                    });
                }
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    *name_span,
                    node.no_field_message(field_name),
                ));
                None
            }
            _ => None,
        }
    }

    /// Emit one key column of a durable operation: lower the inline key expression
    /// (evaluating it here) or read the `place`'s pre-evaluated key slot.
    fn emit_key(&mut self, key: PlaceKey, key_ty: ScalarType, span: SourceSpan) -> Option<()> {
        match key {
            PlaceKey::Expr(expr) => self.lower_as(expr, LTy::bare_scalar(key_ty)),
            PlaceKey::Bound(slot) => {
                self.push(Instr::LocalGet(slot), span);
                Some(())
            }
            // Lower the identity against the addressed root's identity type, then spread it
            // into that root's key columns. The one `Identity` key supplies the whole root
            // key-path, so this pushes every root key column, matching the entry site's key
            // arity. An identity minted over a different root is a type mismatch here.
            PlaceKey::Identity { expr, root, cols } => {
                self.lower_as(
                    expr,
                    LTy::Identity {
                        root,
                        optional: false,
                    },
                )?;
                self.push(Instr::IdentityKeyPath(cols), span);
                Some(())
            }
        }
    }

    /// Emit a durable operation's whole key-path, root column first, so the innermost
    /// key is left on top — the order the kernel's `pop_key_path` reads back to a
    /// root-first path. Path length does not name the node kind: a single-key root is
    /// one column and a single-level branch two, but a composite-key root is itself
    /// multi-column.
    pub(super) fn emit_key_path(&mut self, keys: &[DurKey], span: SourceSpan) -> Option<()> {
        for column in keys {
            self.emit_key(column.key, column.key_ty, span)?;
        }
        Some(())
    }

    /// Capture the root key-path an entry identity supplies into one pre-evaluated
    /// slot per root key column (root-first). The identity stands for the whole root
    /// key tuple, so a whole-entry write through it — which reads (`DurExists`) and
    /// writes off the same columns several times — evaluates the identity once here
    /// and reuses the slots, exactly as an inline key tuple is captured. Returns the
    /// slots in root-column order.
    pub(super) fn capture_identity_key_slots(
        &mut self,
        expr: &Expression,
        root: u16,
        cols: u16,
        span: SourceSpan,
    ) -> Option<Vec<u16>> {
        self.lower_as(
            expr,
            LTy::Identity {
                root,
                optional: false,
            },
        )?;
        self.push(Instr::IdentityKeyPath(cols), span);
        let cols = cols as usize;
        // `IdentityKeyPath` leaves the columns root-first, so the last column is on top;
        // pop into slots from the last column back so each slot holds its own column.
        let mut slots = vec![0u16; cols];
        for column in (0..cols).rev() {
            let slot = self.alloc_slot(expr.span())?;
            self.push(Instr::LocalSet(slot), span);
            slots[column] = slot;
        }
        Some(slots)
    }

    /// Capture an entry identity into one pre-evaluated `(slot, scalar)` column per root key
    /// column (root-first): the slots from [`capture_identity_key_slots`] paired with the
    /// addressed root's key scalars. The single owner for recording an identity operand as a
    /// place/traversal key-path, so a place binding and a traversal ancestor spread it
    /// identically.
    pub(super) fn capture_identity_key_columns(
        &mut self,
        expr: &Expression,
        root: u16,
        cols: u16,
        span: SourceSpan,
    ) -> Option<Vec<(u16, ScalarType)>> {
        let slots = self.capture_identity_key_slots(expr, root, cols, span)?;
        // The RootId was resolved from a root in this registry when the identity column was
        // built, so it is present here.
        #[allow(
            clippy::expect_used,
            reason = "lowering invariant: an identity operand's RootId names a root in this registry"
        )]
        let scalars = self
            .durable
            .root_by_id(root)
            .expect("an identity operand's root is registered")
            .key
            .clone();
        Some(slots.into_iter().zip(scalars).collect())
    }

    /// Materialize a durable operation's whole key-path into one pre-evaluated slot per
    /// physical key column (root-first) — the capture a read-modify-write or an upsert
    /// needs so its several ops key off one evaluation. A `Bound` column reuses the place
    /// slot it already holds; an `Expr` column is evaluated once into a fresh slot; an
    /// entry-identity column spreads into the addressed root's columns through
    /// [`capture_identity_key_slots`], so a single identity operand yields one slot per
    /// root key column. The returned slots are the physical key-path in column order.
    fn capture_key_slots(&mut self, keys: &[DurKey], span: SourceSpan) -> Option<Vec<u16>> {
        let mut slots = Vec::with_capacity(keys.len());
        for column in keys {
            match column.key {
                PlaceKey::Bound(slot) => slots.push(slot),
                PlaceKey::Expr(expr) => {
                    let slot = self.alloc_slot(expr.span())?;
                    self.emit_key(column.key, column.key_ty, span)?;
                    self.push(Instr::LocalSet(slot), span);
                    slots.push(slot);
                }
                PlaceKey::Identity { expr, root, cols } => {
                    slots.extend(self.capture_identity_key_slots(expr, root, cols, span)?);
                }
            }
        }
        Some(slots)
    }

    /// Lower `place name = ^root(key)`: evaluate the entry address's key tuple
    /// exactly once into a fresh local slot and record the binding. The binding is
    /// immutable and does not shadow an existing name; the target must be a whole
    /// durable entry address (not a field, another place, or a non-durable value).
    /// A place over a not-yet-executable root reports the same trough diagnostic as
    /// an inline operation over it.
    pub(super) fn lower_place_binding(
        &mut self,
        name: &str,
        name_span: SourceSpan,
        place_expr: &Expression,
    ) {
        if is_reserved_builtin_name(name) {
            self.fail(reserved_builtin_name(self.file, name_span, name));
            return;
        }
        if self.lookup(name).is_some() || self.lookup_place(name).is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                name_span,
                format!("`{name}` is already bound in this scope"),
            ));
            return;
        }
        if self.bind_place_address(name, place_expr).is_none() {
            // The address did not resolve (a dropped root, a bad key, a non-entry
            // target); its own diagnostic already fired. Poison the name so its later
            // uses do not each re-report an unbound place on top of that cause.
            self.poisoned_bindings.insert(name.to_string());
        }
    }

    /// Bind a validated `place` name to its durable entry address, pushing the
    /// [`PlaceLocal`] on success. Returns `None` when the address does not resolve — the
    /// resolver has already reported why — so the caller can poison the name.
    fn bind_place_address(&mut self, name: &str, place_expr: &Expression) -> Option<()> {
        if !matches!(self.durable_access(place_expr), Some(DurShape::Entry)) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                place_expr.span(),
                "a `place` names a whole durable entry address such as `^root(key)`".to_string(),
            ));
            return None;
        }
        let place = self.resolve_durable(place_expr)?;
        let span = place.span;
        let DurTarget::Entry {
            entry_site,
            record,
            node_kind,
        } = place.target
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                place_expr.span(),
                "a `place` names a whole durable entry address such as `^root(key)`, not a field"
                    .to_string(),
            ));
            return None;
        };
        // Evaluate each key column of the address exactly once into a fresh slot, root
        // column first, so every later operation through the place reads the slots rather
        // than re-running the key operands. A branch place binds its whole key-path, and an
        // identity operand at the root position spreads into the root's key columns — a
        // composite-key root binds one slot per column from the single identity value.
        let mut key_slots = Vec::with_capacity(place.keys.len());
        for column in place.keys {
            match column.key {
                PlaceKey::Expr(key_expr) => {
                    let key_slot = self.alloc_slot(key_expr.span())?;
                    self.lower_as(key_expr, LTy::bare_scalar(column.key_ty))?;
                    self.push(Instr::LocalSet(key_slot), span);
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
                        Code::CheckType.as_str(),
                        self.file,
                        place_expr.span(),
                        "a `place` names a store address `^root(key)`, not another place"
                            .to_string(),
                    ));
                    return None;
                }
            }
        }
        self.places.push(PlaceLocal {
            name: name.to_string(),
            key_slots,
            entry_site,
            record,
            node_kind,
        });
        Some(())
    }

    /// Resolve a durable place against the store root, reporting a diagnostic on a
    /// bad root name, key arity, or field name. The returned place holds no borrow of
    /// the registry.
    pub(super) fn resolve_durable<'e>(&mut self, expr: &'e Expression) -> Option<DurablePlace<'e>> {
        // A place-rooted composed address resolves through the place's pre-evaluated key
        // columns; an inline `^root…` address resolves against the store root below.
        if self.is_place_rooted(expr) {
            return self.resolve_place_composed(expr);
        }
        // A durable access names its store at the `^name` leaf. Resolving it here (rather
        // than assuming one store) selects the addressed root and reports a bad name or a
        // parked shape precisely; a non-address expression is cleanly `None`.
        let root_name = saved_root_name(expr)?;
        let root = self.resolve_root(root_name, expr.span())?;
        match expr {
            // A whole-entry address `^root[key].b1[k1]….bn[kn]` at any depth.
            Expression::Keyed { span, .. } => {
                let (keys, node) = self.resolve_entry_address(root, expr)?;
                Some(DurablePlace {
                    keys,
                    target: DurTarget::Entry {
                        entry_site: node.entry_site(),
                        record: node.record(),
                        node_kind: node.place_node_kind(),
                    },
                    span: *span,
                })
            }
            // A field-exact address `<entry-address>.field`, a whole root-level group
            // `<root-address>.group`, or a group-leaf address `<root-address>.group.leaf`.
            Expression::Field {
                base,
                name: field_name,
                name_span,
                span,
                ..
            } => {
                // A group-leaf address: the base resolves to a root-level group, and this
                // selector names one of its leaves. Resolved before the entry-address forms
                // because its base is a group address, not an entry address.
                if let Some((keys, group)) = self.resolve_group_address(root, base) {
                    let Some((slot, leaf)) = group.field_index(field_name) else {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType.as_str(),
                            self.file,
                            *name_span,
                            format!("group `{}` has no field `{field_name}`", group.name),
                        ));
                        return None;
                    };
                    return Some(DurablePlace {
                        keys,
                        target: DurTarget::GroupLeaf {
                            entry_site: group.entry_site,
                            slot,
                            ty: leaf.ty,
                            required: leaf.required,
                        },
                        span: *span,
                    });
                }
                let (keys, node) = self.resolve_entry_address(root, base)?;
                if let Some(field) = node.field(field_name) {
                    let site = self
                        .draft
                        .alloc_site(SiteDef::field_leaf(field.path))
                        .index();
                    return Some(DurablePlace {
                        keys,
                        target: DurTarget::Field {
                            site,
                            ty: field.ty,
                            required: field.required,
                        },
                        span: *span,
                    });
                }
                // A whole root-level group address `^root(k).group`. Groups are executable
                // only at the root level, so only a root node offers one.
                if let DurNode::Root(root) = node
                    && let Some(group) = root.group(field_name)
                {
                    return Some(DurablePlace {
                        keys,
                        target: DurTarget::Group {
                            entry_site: group.entry_site,
                            record: group.record,
                        },
                        span: *span,
                    });
                }
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    *name_span,
                    node.no_field_message(field_name),
                ));
                None
            }
            _ => None,
        }
    }

    /// Resolve a durable group address `^root(k).group` to its root key-path and the
    /// addressed root-level group, or `None` when `expr` is not a group address. Only a
    /// syntactic entry-address base is followed, and only a root node offers groups, so a
    /// field or branch selector resolves cleanly to `None` without a diagnostic — the
    /// caller falls through to the entry-address forms.
    fn resolve_group_address<'e>(
        &mut self,
        root: &'a crate::durable::DurableRoot,
        expr: &'e Expression,
    ) -> Option<(Vec<DurKey<'e>>, &'a crate::durable::DurableGroup)> {
        let Expression::Field { base, name, .. } = expr else {
            return None;
        };
        if !is_entry_address(base) {
            return None;
        }
        let (keys, node) = self.resolve_entry_address(root, base)?;
        let DurNode::Root(root) = node else {
            return None;
        };
        let group = root.group(name)?;
        Some((keys, group))
    }

    /// Resolve a durable whole-entry address expression `^root[key].b1[k1]….bn[kn]` into
    /// its key-path (root-first, one column per hop) and the addressed node, walking the
    /// nested branch chain level by level. Returns `None` on a shape that is not an entry
    /// address, and reports a diagnostic then `None` on a bad root or branch name. The
    /// key-path columns are pushed root-first so the innermost key is on top, the order the
    /// kernel's `pop_key_path` expects.
    pub(super) fn resolve_entry_address<'e>(
        &mut self,
        root: &'a crate::durable::DurableRoot,
        expr: &'e Expression,
    ) -> Option<(Vec<DurKey<'e>>, DurNode<'a>)> {
        let Expression::Keyed {
            base, keys, span, ..
        } = expr
        else {
            return None;
        };
        match &**base {
            // The base case `^root[k1, …]`: the root whole-entry address, one key operand
            // per root key column in declaration order.
            Expression::SavedRoot {
                name,
                span: root_span,
            } => {
                self.check_root_name(root, name, *root_span)?;
                // `^root[id]`: one entry-identity operand supplies the whole root key
                // tuple. The identity is spread into the root's key columns at emit, so a
                // single `Identity` key stands for every root column (including a composite
                // key). Any entry-identity operand takes this path; whether it names *this*
                // root is decided by the identity type check at emit (the addressed root's
                // RootId is the expected identity root). A per-column key list keeps the
                // ordinary scalar path.
                if let [only] = keys.as_slice()
                    && self.identity_operand_root(only).is_some()
                {
                    let columns = vec![DurKey {
                        // The identity is lowered against its own root type, not a scalar, so
                        // the wrapper `key_ty` is unused for this column (both emit and capture
                        // recover the per-column scalars from the spread instead); it carries
                        // the first key column only to satisfy the shared `DurKey` shape.
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
                let (mut columns, parent) = self.resolve_entry_address(root, parent_base)?;
                let Some(branch) = parent.branch(branch_name) else {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        *branch_span,
                        parent.no_branch_message(branch_name),
                    ));
                    return None;
                };
                self.push_key_columns(&mut columns, keys, &branch.key, *span)?;
                Some((columns, DurNode::Branch(branch)))
            }
            _ => None,
        }
    }

    /// Match the positional key operands of one node against its ordered key columns,
    /// pushing one [`DurKey`] per column onto `columns` in declaration order (so the whole
    /// key-path is assembled root-first, column order, the order the kernel expects).
    /// Reports a diagnostic and returns `None` on a wrong operand count. The keyed-access
    /// grammar already forbids a named key, so only arity is checked here.
    fn push_key_columns<'e>(
        &mut self,
        columns: &mut Vec<DurKey<'e>>,
        keys: &'e [Expression],
        key_columns: &[ScalarType],
        span: SourceSpan,
    ) -> Option<()> {
        if keys.len() != key_columns.len() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
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
            self.fail(name_error(self.file, span, name));
            None
        }
    }

    /// The store-root index a key operand names when it is statically an entry identity:
    /// a binding of identity type (`^root[id]`), or an `Id(^root, …)` constructor call
    /// (`^root[Id(^root, k)]`). `None` for any other operand — an ordinary scalar key.
    /// Non-emitting: it only inspects the binding environment and the call spelling.
    fn identity_operand_root(&self, expr: &Expression) -> Option<u16> {
        match expr {
            Expression::Name { segments, .. } => match segments.as_slice() {
                [name] => self.lookup(name).and_then(|local| local.ty.bare_identity()),
                _ => None,
            },
            Expression::Call { callee, .. } => match &**callee {
                Expression::Name { segments, .. } if matches!(segments.as_slice(), [n] if n == "Id") => {
                    Some(0)
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Lower a durable read (`^r(k)` entry, `^r(k).branch(bk)` branch entry, `^r(k).f`
    /// field, or the place forms).
    pub(super) fn lower_durable_read(&mut self, place: DurablePlace) -> Option<LTy> {
        self.emit_key_path(&place.keys, place.span)?;
        Some(match place.target {
            DurTarget::Entry {
                entry_site, record, ..
            } => {
                self.push(Instr::DurReadEntry(entry_site), place.span);
                LTy::Record {
                    ty: record,
                    optional: true,
                }
            }
            DurTarget::Field { site, ty, .. } => {
                self.push(Instr::DurReadField(site), place.span);
                garg_to_lty(ty).to_optional()
            }
            // A whole root-level group materializes as one optional group record: the
            // group's own leaves, present exactly when the entry is present.
            DurTarget::Group { entry_site, record } => {
                self.push(Instr::DurReadGroup(entry_site), place.span);
                LTy::Record {
                    ty: record,
                    optional: true,
                }
            }
            // A group leaf reads as group-read-then-project: materialize the whole group,
            // then project the leaf slot. An absent entry (absent group) short-circuits to
            // a vacant of the leaf's optional type; a present group yields the leaf wrapped
            // optional (a required leaf is `SomeWrap`ped, a sparse leaf already reads `T?`).
            DurTarget::GroupLeaf {
                entry_site,
                slot,
                ty,
                required,
                ..
            } => {
                self.push(Instr::DurReadGroup(entry_site), place.span);
                let result = garg_to_lty(ty).to_optional();
                let to_absent = self.push_branch_present(place.span);
                self.push(Instr::FieldGet(slot), place.span);
                if required {
                    self.push(Instr::SomeWrap, place.span);
                }
                let to_end = self.push_jump(place.span);
                let absent = self.here();
                self.patch(to_absent, absent);
                self.push(Instr::VacantLoad(result.image()), place.span);
                let end = self.here();
                self.patch(to_end, end);
                result
            }
        })
    }

    /// Lower `exists(place)`: the presence of the cell the place addresses, or — when the
    /// argument is a store root or a keyed branch family rather than one addressed cell —
    /// whether that family has at least one payload-bearing child. A specific entry or
    /// field address (`^root(key)`, `^root(key).field`, a named `place`) is a keyed
    /// presence probe; a store root (`^root`) or a keyed branch family (`^root(key).notes`)
    /// is the family-populated probe.
    pub(super) fn lower_exists(&mut self, args: &[Argument], span: SourceSpan) -> Option<LTy> {
        let [arg] = args else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`exists` takes one store place".to_string(),
            ));
            return None;
        };
        // A managed-index read completes the presence family over an index: a unique index
        // is a complete-key probe (the presence half of the `if const` lookup), a nonunique
        // index is scan-only and has no keyed presence probe.
        if let Some(read) = self.resolve_index_read(&arg.value) {
            if read.index.unique {
                return self.lower_index_exists(read.index, read.keys, arg.value.span());
            }
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                arg.value.span(),
                format!(
                    "the non-unique index `{}` is scan-only and has no `exists` probe; scan it \
                     with a `for` head",
                    read.index.name
                ),
            ));
            return None;
        }
        // A family argument (a store root, or a keyed branch family whose tail names a
        // declared branch) is the family-populated probe: it names no immediate child key,
        // so it reuses the traversal place resolver and emits only the ancestor key-path.
        // A scalar-field tail is not a family — it falls through to the keyed cell probe.
        if self.arg_is_family(&arg.value) {
            let target = self.resolve_traversal_place(&arg.value)?;
            self.emit_key_path(&target.ancestor_keys, target.span)?;
            self.push(Instr::DurFamilyExists(target.entry_site), span);
            return Some(LTy::bare_scalar(ScalarType::Bool));
        }
        // A specific addressed cell (an entry or a field) probes that one cell's presence.
        if self.durable_access(&arg.value).is_some() {
            let place = self.resolve_durable(&arg.value)?;
            self.emit_key_path(&place.keys, place.span)?;
            let site = match place.target {
                DurTarget::Entry { entry_site, .. } => entry_site,
                DurTarget::Field { site, .. } => site,
                // A group is markerless — its presence is the entry's presence — so a
                // group-cell presence probe has no distinct meaning yet; a group leaf has no
                // site of its own. Probe the containing entry instead.
                DurTarget::Group { .. } | DurTarget::GroupLeaf { .. } => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckUnsupported.as_str(),
                        self.file,
                        arg.value.span(),
                        "`exists` over a group or a group leaf is not supported; probe the \
                         containing entry `^root(key)`"
                            .to_string(),
                    ));
                    return None;
                }
            };
            self.push(Instr::DurExists(site), place.span);
            return Some(LTy::bare_scalar(ScalarType::Bool));
        }
        // A bare name whose binding failed to resolve (a poisoned `const`/`place`) is
        // already reported at the binding; its `exists` use is a consequence, not a
        // fresh misuse, so it adds no diagnostic.
        if let Expression::Name { segments, .. } = &arg.value
            && let [name] = segments.as_slice()
            && self.poisoned_bindings.contains(name.as_str())
        {
            self.failed = true;
            return None;
        }
        self.fail(SourceDiagnostic::at(
            Code::CheckType.as_str(),
            self.file,
            arg.value.span(),
            "`exists` takes a store place such as `^root(key)`, a field, a store root, or a \
             keyed branch family"
                .to_string(),
        ));
        None
    }

    /// Lower `Id(^root, keys…)`: construct the entry identity of the declared store
    /// root from its explicit key columns, without reading the store. The first
    /// argument is the saved-root reference `^root`; the rest are one value per key
    /// column in declaration order, each checked against that column's scalar type. The
    /// key operands are pushed root-first, then `MakeIdentity` wraps them into the
    /// `Id(^root)` value.
    pub(super) fn lower_identity_ctor(
        &mut self,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        if args.iter().any(|arg| arg.name.is_some()) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`Id` takes positional arguments: a store root then one value per key column"
                    .to_string(),
            ));
            return None;
        }
        let Some((root_arg, key_args)) = args.split_first() else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`Id` takes a store root `^root` then one value per key column".to_string(),
            ));
            return None;
        };
        let Expression::SavedRoot {
            name: root_name,
            span: root_span,
        } = &root_arg.value
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                root_arg.value.span(),
                "`Id`'s first argument is the store root `^root`".to_string(),
            ));
            return None;
        };
        let root = self.resolve_root(root_name, *root_span)?;
        let key_columns = root.key.clone();
        if key_args.len() != key_columns.len() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "`Id(^{root_name}, …)` takes {} key column value(s), one per key column",
                    key_columns.len()
                ),
            ));
            return None;
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
        );
        Some(LTy::Identity {
            root: root.root_id,
            optional: false,
        })
    }

    /// Lower a call in the closed pure text floor: `isEmpty(string): bool`,
    /// `contains(string, string): bool`, `trim(string): string`. One owner for the
    /// whole floor; there is no general string library.
    pub(super) fn lower_text_builtin(
        &mut self,
        name: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let text = LTy::bare_scalar(ScalarType::Text);
        let bool_ty = LTy::bare_scalar(ScalarType::Bool);
        let (arity, instr, result): (usize, Instr, LTy) = match name {
            "isEmpty" => (1, Instr::TextIsEmpty, bool_ty),
            "contains" => (2, Instr::TextContains, bool_ty),
            "trim" => (1, Instr::TextTrim, text),
            #[allow(
                clippy::unreachable,
                reason = "match-arm narrowing: the caller dispatched on this exact set of text-floor builtin names before entering this match"
            )]
            _ => unreachable!("caller matched the text-floor names"),
        };
        if args.len() != arity || args.iter().any(|arg| arg.name.is_some()) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("`{name}` takes {arity} positional string argument(s)"),
            ));
            return None;
        }
        for arg in args {
            self.lower_as(&arg.value, text)?;
        }
        self.push(instr, span);
        Some(result)
    }

    /// Lower a collection-returning text-floor call: `split(text, sep): List[string]`
    /// or `lines(text): List[string]`. Both mint (and reuse) the one `List[string]`
    /// COLLTYPES instantiation and emit the split/lines opcode carrying it; the VM
    /// bounds the result by the same law-9 collection limits `append` observes.
    pub(super) fn lower_text_split(
        &mut self,
        name: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let text = LTy::bare_scalar(ScalarType::Text);
        let arity = if name == "split" { 2 } else { 1 };
        if args.len() != arity || args.iter().any(|arg| arg.name.is_some()) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("`{name}` takes {arity} positional string argument(s)"),
            ));
            return None;
        }
        for arg in args {
            self.lower_as(&arg.value, text)?;
        }
        let result = self
            .records
            .instantiate_list(self.draft, GArg::Scalar(ScalarType::Text));
        let idx = self.accept_resolution(result, span, "this text collection result")?;
        let instr = if name == "split" {
            Instr::TextSplit(idx)
        } else {
            Instr::TextLines(idx)
        };
        self.push(instr, span);
        Some(LTy::Collection {
            idx,
            optional: false,
        })
    }

    /// Lower `join(parts: List[string], sep: string): string`: concatenate the list's
    /// text elements with a separator. A first argument that is not a `List[string]`
    /// is a typed diagnostic; the VM bounds the result by the `run.text_limit`
    /// concatenation ceiling.
    pub(super) fn lower_text_join(&mut self, args: &[Argument], span: SourceSpan) -> Option<LTy> {
        let text = LTy::bare_scalar(ScalarType::Text);
        if args.len() != 2 || args.iter().any(|arg| arg.name.is_some()) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`join` takes 2 positional argument(s): a list of string and a separator"
                    .to_string(),
            ));
            return None;
        }
        let idx = self.collection_arg(&args[0].value)?;
        match self.records.collection_spec(idx) {
            CollSpec::List {
                elem: GArg::Scalar(ScalarType::Text),
            } => {}
            _ => {
                self.fail(unsupported(
                    self.file,
                    args[0].value.span(),
                    "`join` on this type (it joins a list of string)",
                ));
                return None;
            }
        }
        self.lower_as(&args[1].value, text)?;
        self.push(Instr::TextJoin, span);
        Some(text)
    }

    /// Lower an empty-collection constructor `List()`/`Map()` against the expected
    /// type: the expected `Collection` supplies the exact instantiation, so the
    /// constructor emits the `ListNew`/`MapNew` for that COLLTYPES index. A `List()`
    /// against a `Map` type (or the reverse), or against a non-collection type, is a
    /// typed diagnostic.
    /// Lower a collection constructor directed by an expected `List`/`Map` type. An
    /// empty `List()`/`Map()` mints the fresh collection; a variadic `List(a, b, c)`
    /// mints the list and then writes each element in order as a visible append. The
    /// map literal is deferred, so `Map(...)` with arguments is refused.
    pub(super) fn lower_collection_ctor(
        &mut self,
        head: &str,
        args: &[Argument],
        span: SourceSpan,
        expected: LTy,
    ) -> Option<()> {
        let LTy::Collection {
            idx,
            optional: false,
        } = expected
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "`{head}()` constructs a collection, but {} is expected here",
                    expected.spelling(self.records)
                ),
            ));
            return None;
        };
        match (head, self.records.collection_spec(idx)) {
            ("List", CollSpec::List { elem }) => {
                self.push(Instr::ListNew(idx), span);
                let elem = garg_to_lty(elem);
                self.append_list_elements(args, elem, span)
            }
            ("Map", CollSpec::Map { .. }) => {
                if !args.is_empty() {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        "a map is constructed empty with `Map()` and filled with `m[k] = v`; \
                         a map literal is not yet available"
                            .to_string(),
                    ));
                    return None;
                }
                self.push(Instr::MapNew(idx), span);
                Some(())
            }
            _ => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!(
                        "`{head}()` does not construct {}",
                        self.records.collection_spelling(idx)
                    ),
                ));
                None
            }
        }
    }

    /// Write each argument of a variadic `List(...)` as a visible element append, in
    /// source order, onto the freshly minted list already on the stack. The arity is
    /// lexical — one append per argument, no hidden loop — and each element is typed by
    /// the list's element type. A named argument is not a list element.
    fn append_list_elements(
        &mut self,
        args: &[Argument],
        elem: LTy,
        span: SourceSpan,
    ) -> Option<()> {
        for arg in args {
            if arg.name.is_some() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    "`List(...)` takes positional element values, not named arguments".to_string(),
                ));
                return None;
            }
            self.lower_as(&arg.value, elem)?;
            self.push(Instr::ListAppend, span);
        }
        Some(())
    }

    /// Lower a variadic `List(a, b, c)` with no expected type: the element type is
    /// inferred from the first argument and every later argument is checked against it.
    /// The elements evaluate left to right into locals so the minted list can be filled
    /// in source order once its element type is known.
    pub(super) fn lower_list_literal_inferred(
        &mut self,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        #[allow(
            clippy::unreachable,
            reason = "match-arm narrowing: the caller dispatches here only for a builtin whose non-empty argument list it already established"
        )]
        let [first, rest @ ..] = args else {
            unreachable!("caller passes a non-empty argument list");
        };
        if first.name.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`List(...)` takes positional element values, not named arguments".to_string(),
            ));
            return None;
        }
        let elem = self.lower_expr(&first.value)?;
        let Some(elem_garg) = elem.as_garg() else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                first.value.span(),
                format!(
                    "a list element is a value type, found {}",
                    elem.spelling(self.records)
                ),
            ));
            return None;
        };
        let mut slots = Vec::with_capacity(args.len());
        let first_slot = self.alloc_slot(first.value.span())?;
        self.push(Instr::LocalSet(first_slot), span);
        slots.push(first_slot);
        for arg in rest {
            if arg.name.is_some() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    "`List(...)` takes positional element values, not named arguments".to_string(),
                ));
                return None;
            }
            self.lower_as(&arg.value, elem)?;
            let slot = self.alloc_slot(arg.value.span())?;
            self.push(Instr::LocalSet(slot), span);
            slots.push(slot);
        }
        let result = self.records.instantiate_list(self.draft, elem_garg);
        let idx = self.accept_resolution(result, span, "this list literal")?;
        self.push(Instr::ListNew(idx), span);
        for slot in slots {
            self.push(Instr::LocalGet(slot), span);
            self.push(Instr::ListAppend, span);
        }
        Some(LTy::Collection {
            idx,
            optional: false,
        })
    }

    /// Lower `isEmpty(x)` over a string or a finite collection. A string routes to
    /// the text floor; a `List`/`Map` lowers to `length(x) == 0`.
    pub(super) fn lower_is_empty(&mut self, args: &[Argument], span: SourceSpan) -> Option<LTy> {
        let [arg] = args else {
            self.fail(builtin_arity(self.file, span, "isEmpty", 1));
            return None;
        };
        if arg.name.is_some() {
            self.fail(builtin_arity(self.file, span, "isEmpty", 1));
            return None;
        }
        let ty = self.lower_expr(&arg.value)?;
        match ty {
            LTy::Scalar {
                scalar: ScalarType::Text,
                optional: false,
            } => {
                self.push(Instr::TextIsEmpty, span);
                Some(LTy::bare_scalar(ScalarType::Bool))
            }
            LTy::Collection {
                idx,
                optional: false,
            } => {
                let len = match self.records.collection_spec(idx) {
                    CollSpec::List { .. } => Instr::ListLen,
                    CollSpec::Map { .. } => Instr::MapLen,
                };
                self.push(len, span);
                let zero = self.draft.intern_int(0);
                self.push(Instr::ConstLoad(zero.index()), span);
                self.push(Instr::EqInt, span);
                Some(LTy::bare_scalar(ScalarType::Bool))
            }
            _ => {
                self.fail(unsupported(
                    self.file,
                    arg.value.span(),
                    "`isEmpty` on this type (it accepts a string, list, or map)",
                ));
                None
            }
        }
    }

    /// Lower `length(x): int` over a finite collection: the element or entry count.
    /// Lower a local bracket read `xs[i]` / `m[k]`: the base is a local collection and
    /// the read yields the presence-typed optional (`T?` for a list element, `V?` for a
    /// map value), joining the same presence family as sparse durable reads. A list
    /// position is a 1-based key; the literal dead indexes `xs[0]` and `xs[-1]` are
    /// refused with a teaching diagnostic, while a computed out-of-range index yields
    /// absent — Marrow has no out-of-bounds fault class. A `Map<int, V>` key `0` is a
    /// legitimate key, not a dead index.
    pub(super) fn lower_local_bracket_read(
        &mut self,
        base: &Expression,
        keys: &[Expression],
        span: SourceSpan,
    ) -> Option<LTy> {
        let base_ty = self.lower_expr(base)?;
        let LTy::Collection {
            idx,
            optional: false,
        } = base_ty
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                base.span(),
                format!(
                    "a bracket lookup needs a list or map, found {}",
                    base_ty.spelling(self.records)
                ),
            ));
            return None;
        };
        let [key] = keys else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "a local bracket lookup takes exactly one key".to_string(),
            ));
            return None;
        };
        match self.records.collection_spec(idx) {
            CollSpec::List { elem } => {
                if let Some(index_text) = dead_list_index_literal(key) {
                    let label = simple_base_label(base);
                    let message = match label {
                        Some(name) => format!(
                            "`{name}[{index_text}]` names no list position. List positions \
                             count from 1; the first element is `{name}[1]`"
                        ),
                        None => format!(
                            "`[{index_text}]` names no list position. List positions count \
                             from 1; the first element is at position 1"
                        ),
                    };
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        key.span(),
                        message,
                    ));
                    return None;
                }
                self.lower_as(key, LTy::bare_scalar(ScalarType::Int))?;
                self.push(Instr::ListIndex, span);
                Some(garg_to_lty(elem).to_optional())
            }
            CollSpec::Map { key: key_ty, value } => {
                self.lower_as(key, garg_to_lty(key_ty))?;
                self.push(Instr::MapGet, span);
                Some(garg_to_lty(value).to_optional())
            }
        }
    }

    /// Lower a local keyed write `m[k] = value`: on a `var` map binding, create or
    /// replace the value at the key (total, except the `run.collection_limit` growth
    /// fault), lowered as a read-modify-write with value semantics — the same shape as
    /// a durable keyed write, differing only by the absent `^`. A `const` binding gets
    /// the ordinary assignment-to-const rejection. A list has no keyed write: `xs[i] =
    /// value` is refused with a teaching diagnostic naming `append` and `Map<int, T>`.
    /// One bracket group on a bare local binding; a nested or compound base is deferred.
    pub(super) fn lower_local_bracket_write(
        &mut self,
        base: &Expression,
        keys: &[Expression],
        span: SourceSpan,
        value: &Expression,
    ) {
        let Expression::Name {
            segments,
            span: base_span,
            ..
        } = base
        else {
            self.fail(unsupported(
                self.file,
                base.span(),
                "this assignment target",
            ));
            return;
        };
        let [name] = segments.as_slice() else {
            self.fail(unsupported(self.file, *base_span, "this assignment target"));
            return;
        };
        let Some(local) = self.lookup(name) else {
            self.fail(name_error(self.file, *base_span, name));
            return;
        };
        let (slot, ty, mutable) = (local.slot, local.ty, local.mutable);
        let LTy::Collection {
            idx,
            optional: false,
        } = ty
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                *base_span,
                format!(
                    "a bracket assignment needs a list or map, found {}",
                    ty.spelling(self.records)
                ),
            ));
            return;
        };
        let [key] = keys else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "a local bracket assignment takes exactly one key".to_string(),
            ));
            return;
        };
        match self.records.collection_spec(idx) {
            CollSpec::Map {
                key: key_ty,
                value: value_ty,
            } => {
                if !mutable {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        *base_span,
                        format!("`{name}` is a `const` and cannot be reassigned"),
                    ));
                    return;
                }
                self.push(Instr::LocalGet(slot), span);
                if self.lower_as(key, garg_to_lty(key_ty)).is_none() {
                    return;
                }
                if self.lower_as(value, garg_to_lty(value_ty)).is_none() {
                    return;
                }
                self.push(Instr::MapInsert, span);
                self.push(Instr::LocalSet(slot), span);
            }
            CollSpec::List { elem } => {
                let rhs = simple_value_spelling(value).unwrap_or_else(|| "_".to_string());
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!(
                        "`{name}` is a list, and a list has no keyed write. Grow it with \
                         `append({name}, {rhs})`, or use a `Map<int, {}>` for replacement at a \
                         position",
                        garg_to_lty(elem).spelling(self.records)
                    ),
                ));
            }
        }
    }

    /// Lower `unset m[k]`: remove a key from a local map, idempotent on an absent key.
    /// The base names a mutable local map; the key is coerced to the map key type and a
    /// `MapRemove` read-modify-writes the local. A list has no keyed removal — a dense
    /// list holds no holes — so `unset xs[i]` is refused with a teaching diagnostic.
    pub(super) fn lower_local_bracket_unset(
        &mut self,
        base: &Expression,
        keys: &[Expression],
        span: SourceSpan,
    ) {
        let Expression::Name {
            segments,
            span: base_span,
            ..
        } = base
        else {
            self.fail(unsupported(self.file, base.span(), "this `unset` target"));
            return;
        };
        let [name] = segments.as_slice() else {
            self.fail(unsupported(self.file, *base_span, "this `unset` target"));
            return;
        };
        let Some(local) = self.lookup(name) else {
            self.fail(name_error(self.file, *base_span, name));
            return;
        };
        let (slot, ty, mutable) = (local.slot, local.ty, local.mutable);
        let LTy::Collection {
            idx,
            optional: false,
        } = ty
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                *base_span,
                format!(
                    "a bracket removal needs a map, found {}",
                    ty.spelling(self.records)
                ),
            ));
            return;
        };
        let [key] = keys else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "a local bracket removal takes exactly one key".to_string(),
            ));
            return;
        };
        match self.records.collection_spec(idx) {
            CollSpec::Map { key: key_ty, .. } => {
                if !mutable {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        *base_span,
                        format!("`{name}` is a `const` and cannot be modified"),
                    ));
                    return;
                }
                self.push(Instr::LocalGet(slot), span);
                if self.lower_as(key, garg_to_lty(key_ty)).is_none() {
                    return;
                }
                self.push(Instr::MapRemove, span);
                self.push(Instr::LocalSet(slot), span);
            }
            CollSpec::List { elem } => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!(
                        "`{name}` is a list, and a list has no keyed removal — a dense list \
                         holds no holes. Use a `Map<int, {}>` when a position may be removed",
                        garg_to_lty(elem).spelling(self.records)
                    ),
                ));
            }
        }
    }

    pub(super) fn lower_length(&mut self, args: &[Argument], span: SourceSpan) -> Option<LTy> {
        let [arg] = args else {
            self.fail(builtin_arity(self.file, span, "length", 1));
            return None;
        };
        if arg.name.is_some() {
            self.fail(builtin_arity(self.file, span, "length", 1));
            return None;
        }
        let idx = self.collection_arg(&arg.value)?;
        let len = match self.records.collection_spec(idx) {
            CollSpec::List { .. } => Instr::ListLen,
            CollSpec::Map { .. } => Instr::MapLen,
        };
        self.push(len, span);
        Some(LTy::bare_scalar(ScalarType::Int))
    }

    /// Lower `append(list, value): List<T>`: append `value` after the last element,
    /// yielding the grown list (collections are values). A non-list first argument,
    /// or a `value` not of the element type, is a typed diagnostic.
    pub(super) fn lower_append(&mut self, args: &[Argument], span: SourceSpan) -> Option<LTy> {
        let [list_arg, value_arg] = args else {
            self.fail(builtin_arity(self.file, span, "append", 2));
            return None;
        };
        if args.iter().any(|arg| arg.name.is_some()) {
            self.fail(builtin_arity(self.file, span, "append", 2));
            return None;
        }
        let idx = self.collection_arg(&list_arg.value)?;
        let CollSpec::List { elem } = self.records.collection_spec(idx) else {
            self.fail(unsupported(
                self.file,
                list_arg.value.span(),
                "`append` on a map (a map is updated with `insert`)",
            ));
            return None;
        };
        self.lower_as(&value_arg.value, garg_to_lty(elem))?;
        self.push(Instr::ListAppend, span);
        Some(LTy::Collection {
            idx,
            optional: false,
        })
    }

    /// Lower an expression that must be a bare collection, returning its COLLTYPES
    /// index. A non-collection value is a typed diagnostic.
    fn collection_arg(&mut self, expr: &Expression) -> Option<u16> {
        let ty = self.lower_expr(expr)?;
        match ty {
            LTy::Collection {
                idx,
                optional: false,
            } => Some(idx),
            other => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    expr.span(),
                    format!(
                        "expected a list or map here, found {}",
                        other.spelling(self.records)
                    ),
                ));
                None
            }
        }
    }

    /// Lower a temporal constructor `date("…")` / `instant("…")` / `duration("…")`.
    /// Construction is from exactly one static string literal, validated and folded
    /// at compile time: a malformed or out-of-range canonical form is a typed
    /// `check.type` diagnostic here, so no ordinary program produces an out-of-range
    /// temporal value at runtime. The folded raw scalar is interned as a temporal
    /// constant. `marrow-temporal` owns the canonical text grammar.
    pub(super) fn lower_temporal_construct(
        &mut self,
        scalar: ScalarType,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let spelling = scalar.spelling();
        let [arg] = args else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("`{spelling}` takes one string-literal argument"),
            ));
            return None;
        };
        if arg.name.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                arg.value.span(),
                format!("the `{spelling}` argument is positional"),
            ));
            return None;
        }
        // A temporal value is constructed only from a static string literal, so its
        // canonical form is validated once at compile time rather than parsed at
        // runtime (there is no ambient clock or runtime temporal parse in the floor).
        let Expression::Literal {
            kind: LiteralKind::String,
            text,
            span: arg_span,
        } = &arg.value
        else {
            self.fail(unsupported(
                self.file,
                arg.value.span(),
                &format!("constructing a `{spelling}` from a non-literal value"),
            ));
            return None;
        };
        let Ok(decoded) = decode_string_literal(text) else {
            self.fail(unsupported(self.file, *arg_span, "this string literal"));
            return None;
        };
        let bytes = decoded.as_bytes();
        let const_id = match scalar {
            ScalarType::Date => match marrow_temporal::parse_date(bytes) {
                Some(days) => self.draft.intern_date(days),
                None => return self.fail_temporal_literal(scalar, &decoded, *arg_span),
            },
            ScalarType::Instant => match marrow_temporal::parse_instant(bytes) {
                Some(nanos) => self.draft.intern_instant(nanos),
                None => return self.fail_temporal_literal(scalar, &decoded, *arg_span),
            },
            ScalarType::Duration => match marrow_temporal::parse_duration(bytes) {
                Some(nanos) => self.draft.intern_duration(nanos),
                None => return self.fail_temporal_literal(scalar, &decoded, *arg_span),
            },
            #[allow(
                clippy::unreachable,
                reason = "match-arm narrowing: the caller restricts this dispatch to the temporal scalar types matched above"
            )]
            _ => unreachable!("caller passes only a temporal scalar"),
        };
        self.push(Instr::ConstLoad(const_id.index()), span);
        Some(LTy::bare_scalar(scalar))
    }

    /// Report a malformed or out-of-range temporal literal and return `None`.
    fn fail_temporal_literal(
        &mut self,
        scalar: ScalarType,
        value: &str,
        span: SourceSpan,
    ) -> Option<LTy> {
        let form = match scalar {
            ScalarType::Date => "a canonical date `YYYY-MM-DD` in years 0001-9999",
            ScalarType::Instant => {
                "a canonical UTC instant `YYYY-MM-DDTHH:MM:SS[.fraction]Z` in years 0001-9999"
            }
            ScalarType::Duration => "a canonical duration `[-]PT<seconds>[.fraction]S`",
            #[allow(
                clippy::unreachable,
                reason = "match-arm narrowing: the caller restricts this dispatch to the temporal scalar types matched above"
            )]
            _ => unreachable!("caller passes only a temporal scalar"),
        };
        self.fail(SourceDiagnostic::at(
            Code::CheckType.as_str(),
            self.file,
            span,
            format!(
                "`{value}` is not {form}, so it is not a `{}` literal",
                scalar.spelling()
            ),
        ));
        None
    }

    /// Lower `addDays(date, int): date` or `daysBetween(date, date): int`,
    /// emitting the checked temporal instruction after type-checking the operands.
    pub(super) fn lower_date_arith(
        &mut self,
        builtin: Builtin,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let (name, second, instr, result) = match builtin {
            Builtin::DateAddDays => (
                "addDays",
                ScalarType::Int,
                Instr::DateAddDays,
                ScalarType::Date,
            ),
            Builtin::DateDaysBetween => (
                "daysBetween",
                ScalarType::Date,
                Instr::DateDaysBetween,
                ScalarType::Int,
            ),
            #[allow(
                clippy::unreachable,
                reason = "match-arm narrowing: the caller restricts this dispatch to the date-arithmetic builtins matched above"
            )]
            _ => unreachable!("caller passes only a date-arithmetic builtin"),
        };
        let [first_arg, second_arg] = args else {
            self.fail(builtin_arity(self.file, span, name, 2));
            return None;
        };
        if first_arg.name.is_some() || second_arg.name.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("`{name}` arguments are positional"),
            ));
            return None;
        }
        self.expect_bare_scalar(&first_arg.value, ScalarType::Date, name)?;
        self.expect_bare_scalar(&second_arg.value, second, name)?;
        self.push(instr, span);
        Some(LTy::bare_scalar(result))
    }

    /// Lower `expr` and require it to be exactly the bare scalar `expected`, failing
    /// with a `check.type` diagnostic (naming `builtin`) otherwise.
    fn expect_bare_scalar(
        &mut self,
        expr: &Expression,
        expected: ScalarType,
        builtin: &str,
    ) -> Option<()> {
        let ty = self.lower_expr(expr)?;
        if ty == LTy::bare_scalar(expected) {
            return Some(());
        }
        self.fail(SourceDiagnostic::at(
            Code::CheckType.as_str(),
            self.file,
            expr.span(),
            format!(
                "`{builtin}` expects a `{}` argument, found `{}`",
                expected.spelling(),
                ty.spelling(self.records)
            ),
        ));
        None
    }

    pub(super) fn lower_conversion(
        &mut self,
        target: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let [arg] = args else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("`{target}` conversion takes one value"),
            ));
            return None;
        };
        if arg.name.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                arg.value.span(),
                "a conversion argument is positional".to_string(),
            ));
            return None;
        }
        let source = self.lower_expr(&arg.value)?;
        // `string(value)` renders any interpolable value — a scalar, an enum, or an
        // entry identity — to its canonical text, the same rendering interpolation and
        // program output use.
        if target == "string" && is_interpolable(source) {
            self.push(Instr::ConvString, span);
            return Some(LTy::bare_scalar(ScalarType::Text));
        }
        use ScalarType::{Bytes, Text};
        let (instr, result) = match (target, source.bare_scalar_type()) {
            ("bytes", Some(Text)) => (Instr::ConvBytesText, Bytes),
            _ => {
                self.fail(unsupported(
                    self.file,
                    span,
                    &format!("converting {} to {target}", source.spelling(self.records)),
                ));
                return None;
            }
        };
        self.push(instr, span);
        Some(LTy::bare_scalar(result))
    }

    /// Lower `unreachable("static text")`: the sole application-invariant fault. It
    /// takes exactly one static string literal, emits a fault instruction carrying
    /// that text, and diverges (control never continues past it).
    pub(super) fn lower_unreachable(
        &mut self,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<CallResult> {
        let [arg] = args else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`unreachable` takes one static string literal".to_string(),
            ));
            return None;
        };
        if arg.name.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                arg.value.span(),
                "`unreachable` takes one positional static string literal".to_string(),
            ));
            return None;
        }
        let Expression::Literal {
            kind: LiteralKind::String,
            text,
            span: lit_span,
        } = &arg.value
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                arg.value.span(),
                "`unreachable` requires a static string literal, not a computed value".to_string(),
            ));
            return None;
        };
        let Ok(decoded) = decode_string_literal(text) else {
            self.fail(unsupported(self.file, *lit_span, "this string literal"));
            return None;
        };
        let const_id = self.draft.intern_text(&decoded);
        self.push(Instr::Unreachable(const_id.index()), span);
        Some(CallResult::Diverges)
    }

    /// Lower `todo("static text")`: a deferred path the author has not implemented. It
    /// mirrors `unreachable` exactly — one static string literal, a fault instruction
    /// carrying that text, and divergence — but raises `run.todo` when reached.
    pub(super) fn lower_todo(&mut self, args: &[Argument], span: SourceSpan) -> Option<CallResult> {
        let [arg] = args else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`todo` takes one static string literal".to_string(),
            ));
            return None;
        };
        if arg.name.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                arg.value.span(),
                "`todo` takes one positional static string literal".to_string(),
            ));
            return None;
        }
        let Expression::Literal {
            kind: LiteralKind::String,
            text,
            span: lit_span,
        } = &arg.value
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                arg.value.span(),
                "`todo` requires a static string literal, not a computed value".to_string(),
            ));
            return None;
        };
        let Ok(decoded) = decode_string_literal(text) else {
            self.fail(unsupported(self.file, *lit_span, "this string literal"));
            return None;
        };
        let const_id = self.draft.intern_text(&decoded);
        self.push(Instr::Todo(const_id.index()), span);
        Some(CallResult::Diverges)
    }

    /// Lower a durable assignment: a whole-entry upsert (root or branch) or a root
    /// field set.
    pub(super) fn lower_durable_assign(&mut self, place: DurablePlace, value: &Expression) {
        match place.target {
            DurTarget::Entry {
                entry_site, record, ..
            } => {
                let root_slot = place.root_bound_slot();
                if self
                    .lower_upsert(&place.keys, entry_site, record, value, place.span)
                    .is_some()
                    && let Some(slot) = root_slot
                {
                    // A root upsert leaves the root entry present on every path from
                    // here, so subsequent sparse sets through the root place lower to the
                    // strict form. A key-path with more than one bound key slot — whether a
                    // branch or a composite-key root — has no single root slot here and so
                    // marks nothing; a guarded set through such a place uses the
                    // `exists`/`if const` presence path instead.
                    self.mark_present(vec![slot]);
                }
            }
            DurTarget::Field { site, ty, required } => {
                // A sparse set through a `place` a presence fact dominates lowers to the
                // strict present-entry form: it reads the containing entry's whole
                // key-path from the place's pre-evaluated slots and asserts the entry is
                // present, so it pushes no key operand. A root or a branch field is
                // handled uniformly by the key-path. Every other field set keeps the bare
                // form (create-or-reconcile at commit for a sparse set).
                let bare = garg_to_lty(ty);
                if !required
                    && let Some(key_slots) = place.bound_key_path()
                    && self.is_present_path(&key_slots)
                {
                    let expected = bare.to_optional();
                    if self.lower_as(value, expected).is_none() {
                        return;
                    }
                    self.push(Instr::DurSetSparsePresent { site, key_slots }, place.span);
                    return;
                }
                if self.emit_key_path(&place.keys, place.span).is_none() {
                    return;
                }
                let expected = if required { bare } else { bare.to_optional() };
                if self.lower_as(value, expected).is_none() {
                    return;
                }
                let instr = if required {
                    Instr::DurSetRequired(site)
                } else {
                    Instr::DurSetSparse(site)
                };
                self.push(instr, place.span);
            }
            // `^root(k).group = R.group(…)`: an exact whole-group replacement, group-scoped
            // (the entry's other groups, top-level fields, and branches are untouched). The
            // key-path is pushed first, then the group record, the order `DurReplaceGroup`
            // reads. A replace over an absent entry is Missing and touches nothing — a group
            // is a value unit of an existing entry, never created on its own.
            DurTarget::Group { entry_site, record } => {
                if self.emit_key_path(&place.keys, place.span).is_none() {
                    return;
                }
                if self
                    .lower_as(
                        value,
                        LTy::Record {
                            ty: record,
                            optional: false,
                        },
                    )
                    .is_none()
                {
                    return;
                }
                self.push(Instr::DurReplaceGroup(entry_site), place.span);
            }
            // `^root(k).group.leaf = value`: a whole-group read-modify-write.
            DurTarget::GroupLeaf {
                entry_site,
                slot,
                ty,
                ..
            } => {
                self.lower_group_leaf_rmw(
                    &place.keys,
                    entry_site,
                    slot,
                    GroupLeafEdit::Set { value, ty },
                    place.span,
                );
            }
        }
    }

    /// Lower a group-leaf read-modify-write `^root(k).group.leaf = value` or
    /// `delete ^root(k).group.leaf`: evaluate the key-path once into slots, read the whole
    /// group, and — only when the entry (and so the group) is present — rewrite the leaf
    /// slot (set present, or unset to vacant) on the materialized group record and replace
    /// the whole group. An absent entry short-circuits to a no-op: a group is a value unit
    /// of an existing entry, never created on its own. The group is materialized whole and
    /// written back, so a sibling leaf is preserved.
    fn lower_group_leaf_rmw(
        &mut self,
        keys: &[DurKey],
        entry_site: u16,
        slot: u16,
        edit: GroupLeafEdit,
        span: SourceSpan,
    ) -> Option<()> {
        // Evaluate each key column once into a fresh slot (root-first) so the read and the
        // replace key off the same evaluated columns. A group is a root-level value unit, so
        // its key-path is the root's — an identity operand spreads into the root's columns.
        let key_slots = self.capture_key_slots(keys, span)?;
        // A set evaluates its bare leaf value once into a slot before the read, so the read
        // record is on top of the stack when the leaf op runs.
        let value_slot = match &edit {
            GroupLeafEdit::Set { value, ty } => {
                let value_slot = self.alloc_slot(span)?;
                self.lower_as(value, garg_to_lty(*ty))?;
                self.push(Instr::LocalSet(value_slot), span);
                Some(value_slot)
            }
            GroupLeafEdit::Unset => None,
        };
        // Read the group; present -> its materialized record is on the stack and the write
        // back runs; absent -> jump past the write back, a clean no-op (the group was never
        // there to modify).
        self.emit_slots(&key_slots, span);
        self.push(Instr::DurReadGroup(entry_site), span);
        let to_end = self.push_branch_present(span);
        // Present: rewrite the leaf slot on the materialized record, then replace the group.
        match edit {
            GroupLeafEdit::Set { .. } => {
                #[allow(
                    clippy::expect_used,
                    reason = "lowering bookkeeping: a `Set` edit lowers its value expression before this emit, so its result slot is bound"
                )]
                self.push(
                    Instr::LocalGet(value_slot.expect("a set evaluates its value")),
                    span,
                );
                self.push(Instr::FieldSet(slot), span);
            }
            GroupLeafEdit::Unset => {
                self.push(Instr::FieldUnset(slot), span);
            }
        }
        let rec_slot = self.alloc_slot(span)?;
        self.push(Instr::LocalSet(rec_slot), span);
        self.emit_slots(&key_slots, span);
        self.push(Instr::LocalGet(rec_slot), span);
        self.push(Instr::DurReplaceGroup(entry_site), span);
        let end = self.here();
        self.patch(to_end, end);
        Some(())
    }

    /// Lower `^r(k) = record` or `^r(k).branch(bk) = Resource.branch(...)` to the
    /// transaction-local presence branch (design §D): `DurExists` over the entry's whole
    /// key-path decides `replace` vs `create` against the coherent staged view. The
    /// key-path is materialized into slots (one per column, root first) so the exists,
    /// replace, and create ops all key off the same evaluated columns.
    fn lower_upsert(
        &mut self,
        keys: &[DurKey],
        entry_site: u16,
        record: TypeId,
        value: &Expression,
        span: SourceSpan,
    ) -> Option<()> {
        // A bound (place) column already holds its key in a pre-evaluated slot; reuse it
        // so the create/replace ops key off it (the verifier's presence lattice
        // recognizes a root create as establishing that slot's entry). An inline column
        // is evaluated once into a fresh slot. An entry-identity root column spreads into
        // the root's key columns, so the exists/replace/create ops key off the same
        // evaluation whether the whole-entry address is a root (identity or per-column) or
        // a branch below an identity-keyed root.
        let key_slots: Vec<u16> = self.capture_key_slots(keys, span)?;
        let rec_slot = self.alloc_slot(span)?;
        self.lower_as(
            value,
            LTy::Record {
                ty: record,
                optional: false,
            },
        )?;
        self.push(Instr::LocalSet(rec_slot), span);

        self.emit_slots(&key_slots, span);
        self.push(Instr::DurExists(entry_site), span);
        let to_create = self.push_jif(span);
        // Present: replace.
        self.emit_slots(&key_slots, span);
        self.push(Instr::LocalGet(rec_slot), span);
        self.push(Instr::DurReplaceEntry(entry_site), span);
        let to_end = self.push_jump(span);
        // Absent: create.
        let create_at = self.here();
        self.patch(to_create, create_at);
        self.emit_slots(&key_slots, span);
        self.push(Instr::LocalGet(rec_slot), span);
        self.push(Instr::DurCreateEntry(entry_site), span);
        let end = self.here();
        self.patch(to_end, end);
        Some(())
    }

    /// Push a durable operation's key-path from pre-evaluated slots, root column first,
    /// so the innermost key lands on top — the order the kernel's `pop_key_path` reads.
    fn emit_slots(&mut self, slots: &[u16], span: SourceSpan) {
        for slot in slots {
            self.push(Instr::LocalGet(*slot), span);
        }
    }

    /// Lower `delete ^r(k)` / `delete ^r(k).branch(bk)` (entry payload erase) or
    /// `delete ^r(k).f` (sparse-field erase).
    pub(super) fn lower_durable_delete(&mut self, path: &Expression, span: SourceSpan) {
        if self.durable_access(path).is_none() {
            self.fail(unsupported(self.file, span, "this delete target"));
            return;
        }
        let Some(place) = self.resolve_durable(path) else {
            return;
        };
        // A group-leaf clear is a whole-group read-modify-write (its key-path is evaluated
        // inside the helper), so it is handled before the shared single key-path emission.
        if let DurTarget::GroupLeaf {
            entry_site,
            slot,
            required,
            ..
        } = place.target
        {
            if required {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    place.span,
                    "a required group leaf cannot be deleted".to_string(),
                ));
                return;
            }
            self.lower_group_leaf_rmw(
                &place.keys,
                entry_site,
                slot,
                GroupLeafEdit::Unset,
                place.span,
            );
            return;
        }
        let key_path = place.bound_key_path();
        if self.emit_key_path(&place.keys, place.span).is_none() {
            return;
        }
        match place.target {
            DurTarget::Entry { entry_site, .. } => {
                self.push(Instr::DurEraseEntry(entry_site), place.span);
                // The entry's payload is gone; a later sparse set through the same place
                // must not assume presence.
                if let Some(path) = &key_path {
                    self.clear_present_path(path);
                }
            }
            DurTarget::Field { site, required, .. } => {
                if required {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        place.span,
                        "a required field cannot be deleted".to_string(),
                    ));
                    return;
                }
                self.push(Instr::DurEraseField(site), place.span);
            }
            // `delete ^root(k).group`: erase only that group's leaves; the entry's other
            // groups, top-level fields, and branches are untouched.
            DurTarget::Group { entry_site, .. } => {
                self.push(Instr::DurEraseGroup(entry_site), place.span);
            }
            #[allow(
                clippy::unreachable,
                reason = "lowering bookkeeping: a group-leaf delete is dispatched on a dedicated path before this shared key-path emit, so it never reaches this arm"
            )]
            DurTarget::GroupLeaf { .. } => {
                unreachable!("a group-leaf delete is handled before the shared key-path emit")
            }
        }
    }
}
