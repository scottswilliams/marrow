//! The durable Product declaration table and the flat root-occurrence table.
//!
//! A durable **Product** is a declaration: the resource type a `store` root projects.
//! One Product declaration has one canonical member/value graph — its top-level fields,
//! static `group` namespaces, and nested keyed `branch` placements, each with its ledger
//! identity and value shape — and one runtime surface, however many roots occur over it.
//! A **root** is an occurrence: its own placement identity, spelling, key tuple, and
//! managed indexes, referencing the one declaration.
//!
//! The draft therefore holds two flat tables rather than one graph per root: a
//! [`ProductDeclarationTable`] keyed by [`DurableProductIdentity`], and a flat
//! [`RootOccurrence`] row per root that references its declaration. The wire format is
//! unchanged — v0 still carries the full member graph per root — so the encoder projects
//! each occurrence from its one retained declaration. Nothing is retained per
//! (root x member).
//!
//! A declaration's member graph is itself flat: [`ProductDeclarationGraph`] is a table of
//! [`DeclarationNode`] rows, each carrying its parent's ordinal and the contiguous run of
//! rows holding its own direct members. Rows are appended level by level, so a node's
//! members are always one run and a walk follows spans rather than owned child vectors.
//! The wire bytes and the durable contract id are both projected from these rows, so the
//! two derive from one set of facts and cannot drift apart.
//!
//! Both tables publish **selectors**: a completed occurrence row publishes one
//! [`RootOccurrenceSelector`], and every canonical path — a root's own placement, one of
//! its managed indexes, or one Product declaration row — publishes one
//! [`CanonicalDeclarationPathSelector`]. A selector is opaque and carries the exact live
//! row it was published by, so it names a place without exposing an ordinal a caller
//! could write by hand. The pair of them is the only input to the site binder
//! ([`crate::ImageDraft::bind_occurrence_site`]).
//!
//! The occurrence ordinal is not itself secret: [`crate::AdmittedRoot::root_id`] publishes
//! it beside the selectors, because it is the wire RootId an entry identity `Id(^root)`
//! carries and the compiler must emit it into identity instructions. What the selectors
//! establish is that no ordinal a caller holds can *name a place*: the binder accepts
//! selectors only, so a number is a wire value here and never an address.

use std::collections::BTreeMap;

use crate::bounds::MAX_DURABLE_MEMBERS;
use crate::draft::{DraftIdentity, KeyColumn, RowStamp, StrId, TypeId};
use crate::durable_id::{
    DurableIndexShape, DurableProductIdentity, LedgerIdBytes, RootPlacementIdentity,
};
use crate::value_dag::ValueShapeNodeId;

/// The ordinal of one row in a [`ProductDeclarationGraph`].
///
/// It is an index into that one graph's rows and carries no meaning anywhere else: it is
/// not a ledger id, a container-table index, or a site. A graph holds at most
/// [`MAX_DURABLE_MEMBERS`] rows, so the ordinal is a `u16`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct DeclarationNodeOrdinal(u16);

impl DeclarationNodeOrdinal {
    fn index(self) -> usize {
        usize::from(self.0)
    }
}

/// The ordinal of one row in the flat root-occurrence table. It is also the RootId an
/// entry identity `Id(^root)` carries on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RootOccurrenceOrdinal(u16);

impl RootOccurrenceOrdinal {
    fn index(self) -> usize {
        usize::from(self.0)
    }
}

/// The contiguous run of rows holding one node's direct members, in declaration order.
///
/// Rows are appended one level at a time, so every node's members land in one run and a
/// walk of the graph is a slice of the row table rather than a pointer chase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct DeclarationSpan {
    start: u16,
    len: u16,
}

impl DeclarationSpan {
    fn range(self) -> std::ops::Range<usize> {
        let start = usize::from(self.start);
        start..start + usize::from(self.len)
    }
}

/// What one declaration row declares: a stored field with its value shape, a static
/// `group` namespace, or a keyed `branch` placement. A group's and a branch's own members
/// are rows of their own, reached through [`ProductDeclarationGraph::members_of`] — the
/// shape carries no owned member vector, so the graph has exactly one nesting owner and
/// this type cannot express a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclarationMemberShape {
    Field {
        id: LedgerIdBytes,
        required: bool,
        /// This field's stored value shape, as a reference into the draft's one
        /// [`crate::CanonicalValueShapeDag`]. The row carries a reference, never a
        /// shape, so a repeated or deeply shared value costs one `u32` per field.
        value: ValueShapeNodeId,
    },
    Group {
        id: LedgerIdBytes,
    },
    Branch {
        placement: LedgerIdBytes,
        /// The branch's source name, interned for the physical layer and for its
        /// qualified constructor spelling. Carried for the surface, not the durable
        /// identity — a rename preserves the contract id.
        name: StrId,
        /// The branch entry's materialized record type. Carried for the surface, not the
        /// identity (the member value shapes carry the identity).
        record: TypeId,
        keys: Vec<KeyColumn>,
    },
}

/// One flat member command: what the member declares and which earlier command declares
/// the node it nests under. `None` is a direct member of the Product.
///
/// The parent is an index into the command vector this row travels in, and must be
/// strictly less than the row's own index, so a command vector states a forest with every
/// parent before its children and can never state a cycle. It carries no owned member
/// vector, so a caller cannot hand the draft a recursive tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclarationMemberDef {
    pub parent: Option<u16>,
    pub shape: DeclarationMemberShape,
}

/// One row of a Product declaration's flat member graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeclarationNode {
    parent: Option<DeclarationNodeOrdinal>,
    members: DeclarationSpan,
    shape: DeclarationMemberShape,
}

impl DeclarationNode {
    pub(crate) fn shape(&self) -> &DeclarationMemberShape {
        &self.shape
    }
}

/// A flat member command vector that does not state a forest.
///
/// Every command must name a parent that an earlier command declared, and that parent must
/// be a node members may nest under — a static `group` or a keyed `branch`. A field
/// declares no members, so naming one as a parent is not a deep graph but a malformed
/// command vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeclarationCommandError {
    /// The parent index is not strictly less than the command's own index.
    ParentNotEarlier,
    /// The named parent declares no members.
    ParentDeclaresNoMembers,
}

/// One Product declaration's canonical member/value graph, as flat rows.
///
/// The rows are the declaration: the wire projection, the durable contract descriptor,
/// the bound recheck, and the comparison between two occurrences of one Product identity
/// all read them, so there is no second member-graph representation for them to disagree
/// with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProductDeclarationGraph {
    rows: Vec<DeclarationNode>,
    /// The Product's own direct members. Always the leading run, since the level-order
    /// construction places them first.
    members: DeclarationSpan,
    /// The declaration stated more members than [`MAX_DURABLE_MEMBERS`], so the rows are
    /// the bounded prefix that was materialized rather than the whole declaration.
    ///
    /// The construction refuses to materialize an unbounded table for a graph the encoder
    /// will refuse anyway; the encoder reads this and reports the exceeded bound, so a
    /// truncated graph can never be encoded.
    over_member_bound: bool,
}

impl ProductDeclarationGraph {
    /// Build one declaration's graph from a flat command vector.
    ///
    /// Rows are placed level by level: a node's own members are placed together, so they
    /// occupy one contiguous run, and every parent precedes its children. Within one
    /// parent, members keep their command order. The placement is therefore deterministic
    /// and injective over ordered forests: two command vectors state the same declaration
    /// exactly when their rows are equal, whatever order the commands themselves arrived
    /// in.
    pub(crate) fn from_commands(
        commands: Vec<DeclarationMemberDef>,
    ) -> Result<Self, DeclarationCommandError> {
        // Group the commands by the parent they name, checking as we go that each parent
        // is an earlier command that may hold members at all. A command vector wider than
        // the member bound is not malformed — it is a declaration the encoder refuses, so
        // it is admitted here as a bounded prefix that records the overflow.
        let mut children: Vec<Vec<u16>> = vec![Vec::new(); commands.len().min(MAX_DURABLE_MEMBERS)];
        let mut roots: Vec<u16> = Vec::new();
        let over_member_bound = commands.len() > MAX_DURABLE_MEMBERS;
        for (index, command) in commands.iter().enumerate().take(MAX_DURABLE_MEMBERS) {
            let index = u16::try_from(index).expect("the member bound is below u16::MAX");
            let Some(parent) = command.parent else {
                roots.push(index);
                continue;
            };
            if parent >= index {
                return Err(DeclarationCommandError::ParentNotEarlier);
            }
            if matches!(
                commands[usize::from(parent)].shape,
                DeclarationMemberShape::Field { .. }
            ) {
                return Err(DeclarationCommandError::ParentDeclaresNoMembers);
            }
            children[usize::from(parent)].push(index);
        }

        // Place the rows breadth-first: each pending node's whole child run is placed at
        // once, so a node's members are exactly one contiguous span of the row table.
        let mut shapes: Vec<Option<DeclarationMemberShape>> = commands
            .into_iter()
            .take(MAX_DURABLE_MEMBERS)
            .map(|command| Some(command.shape))
            .collect();
        let mut graph = Self {
            rows: Vec::with_capacity(shapes.len()),
            members: DeclarationSpan::default(),
            over_member_bound,
        };
        let mut pending: std::collections::VecDeque<(Option<DeclarationNodeOrdinal>, Vec<u16>)> =
            std::collections::VecDeque::new();
        pending.push_back((None, roots));
        let mut placed: Vec<Option<DeclarationNodeOrdinal>> = vec![None; shapes.len()];
        let mut first_level = true;
        while let Some((parent, level)) = pending.pop_front() {
            let span = graph.place_level(parent, &level, &mut shapes, &mut placed);
            if first_level {
                graph.members = span;
                first_level = false;
            } else if let Some(parent) = parent {
                graph.rows[parent.index()].members = span;
            }
            for command in level {
                let taken = std::mem::take(&mut children[usize::from(command)]);
                if !taken.is_empty() {
                    let ordinal = placed[usize::from(command)]
                        .expect("the command was placed in the level just written");
                    pending.push_back((Some(ordinal), taken));
                }
            }
        }
        Ok(graph)
    }

    /// Place one node's direct members as a contiguous run and record where each command
    /// landed, so the next level can name its parent by row ordinal. Returns the run.
    fn place_level(
        &mut self,
        parent: Option<DeclarationNodeOrdinal>,
        level: &[u16],
        shapes: &mut [Option<DeclarationMemberShape>],
        placed: &mut [Option<DeclarationNodeOrdinal>],
    ) -> DeclarationSpan {
        let start = u16::try_from(self.rows.len()).expect("the command count is bounded");
        let mut len = 0u16;
        for command in level {
            let ordinal = DeclarationNodeOrdinal(
                u16::try_from(self.rows.len()).expect("the command count is bounded"),
            );
            let shape = shapes[usize::from(*command)]
                .take()
                .expect("each command is placed exactly once");
            self.rows.push(DeclarationNode {
                parent,
                members: DeclarationSpan::default(),
                shape,
            });
            placed[usize::from(*command)] = Some(ordinal);
            len += 1;
        }
        DeclarationSpan { start, len }
    }

    /// The Product's direct members, in declaration order.
    pub(crate) fn members(&self) -> &[DeclarationNode] {
        &self.rows[self.members.range()]
    }

    /// One node's direct members, in declaration order. A field declares none.
    pub(crate) fn members_of(&self, node: &DeclarationNode) -> &[DeclarationNode] {
        &self.rows[node.members.range()]
    }

    /// Every row, in the order they were placed: a parent always precedes its children.
    pub(crate) fn rows(&self) -> &[DeclarationNode] {
        &self.rows
    }

    /// The declaration exceeded [`MAX_DURABLE_MEMBERS`] and its rows are a bounded
    /// prefix.
    pub(crate) fn over_member_bound(&self) -> bool {
        self.over_member_bound
    }

    /// The ordinals of the Product's direct members, in declaration order.
    fn member_ordinals(&self) -> impl Iterator<Item = DeclarationNodeOrdinal> + use<> {
        Self::ordinals(self.members)
    }

    /// The ordinals of one node's direct members, in declaration order.
    fn member_ordinals_of(
        &self,
        node: DeclarationNodeOrdinal,
    ) -> impl Iterator<Item = DeclarationNodeOrdinal> + use<> {
        Self::ordinals(self.rows[node.index()].members)
    }

    fn ordinals(span: DeclarationSpan) -> impl Iterator<Item = DeclarationNodeOrdinal> + use<> {
        span.range()
            .map(|row| DeclarationNodeOrdinal(u16::try_from(row).expect("rows are bounded")))
    }

    /// The row at `ordinal`, or `None` if the graph has no such row.
    fn row(&self, ordinal: DeclarationNodeOrdinal) -> Option<&DeclarationNode> {
        self.rows.get(ordinal.index())
    }

    /// The chain of rows from a top-level member down to `ordinal`, outermost first.
    ///
    /// A parent always precedes its children, so walking the parent ordinals up and
    /// reversing is bounded by the graph's own nesting depth — the bound
    /// [`crate::encode`]'s recheck already enforces — and needs no descent.
    fn ancestry(&self, ordinal: DeclarationNodeOrdinal) -> Vec<&DeclarationNode> {
        let mut chain = Vec::new();
        let mut cursor = Some(ordinal);
        while let Some(at) = cursor {
            let Some(node) = self.rows.get(at.index()) else {
                break;
            };
            chain.push(node);
            cursor = node.parent;
        }
        chain.reverse();
        chain
    }

    /// Each row's nesting depth, 1 for a top-level member.
    ///
    /// A parent always precedes its children, so one forward pass over the parent
    /// ordinals fixes every depth; no descent of the graph is needed to check the nesting
    /// bound.
    pub(crate) fn depths(&self) -> Vec<usize> {
        let mut depths = Vec::with_capacity(self.rows.len());
        for node in &self.rows {
            let depth = match node.parent {
                Some(parent) => depths[parent.index()] + 1,
                None => 1,
            };
            depths.push(depth);
        }
        depths
    }
}

/// The exact comparison boundary between two occurrences of one Product declaration:
/// its ledger identity and its canonical resource member/value graph.
///
/// The graph is the whole member row table — every field id, `required` flag, and value
/// shape, every static `group`, and every nested keyed `branch` placement together with
/// its key tuple and its own members. Those are Product facts: they are declared by the
/// resource, not by the root that projects it. The *outer* root's placement identity,
/// spelling, key tuple, and managed indexes are occurrence facts and are deliberately not
/// in the claim, so two roots may share a Product while carrying different placements,
/// key tuples, and index shapes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DurableProductClaim {
    identity: DurableProductIdentity,
    graph: ProductDeclarationGraph,
}

impl DurableProductClaim {
    pub(crate) fn new(identity: DurableProductIdentity, graph: ProductDeclarationGraph) -> Self {
        Self { identity, graph }
    }

    pub(crate) fn identity(&self) -> DurableProductIdentity {
        self.identity
    }

    pub(crate) fn graph(&self) -> &ProductDeclarationGraph {
        &self.graph
    }
}

/// The runtime shape one Product declaration binds: the materialized entry record its
/// roots read and write.
///
/// A nested branch's runtime facts — its placement, interned name, and entry record type
/// — are carried by the declaration's own member graph
/// ([`DeclarationMemberShape::Branch`]), which is already part of [`DurableProductClaim`]
/// and compared with it. They are not copied here: the root entry record is the one such
/// fact the member graph does not carry, because it belongs to the resource rather than
/// to any one member.
///
/// These facts are on the wire in the DURABLE section but are excluded from the durable
/// contract-id preimage, so binding them adds no contract bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProductEntryRecordClaim {
    root_entry_record: TypeId,
}

impl ProductEntryRecordClaim {
    pub(crate) fn new(root_entry_record: TypeId) -> Self {
        Self { root_entry_record }
    }

    pub(crate) fn root_entry_record(&self) -> TypeId {
        self.root_entry_record
    }
}

/// One admitted Product declaration: its claim, the runtime surface bound with it, and
/// the stamp that distinguishes this row from a later row deterministically reusing its
/// ordinal.
#[derive(Debug, Clone)]
pub(crate) struct ProductDeclaration {
    claim: DurableProductClaim,
    surface: ProductEntryRecordClaim,
    stamp: RowStamp,
}

impl ProductDeclaration {
    /// This declaration's canonical member/value graph, as flat rows.
    pub(crate) fn graph(&self) -> &ProductDeclarationGraph {
        self.claim.graph()
    }

    pub(crate) fn identity(&self) -> DurableProductIdentity {
        self.claim.identity()
    }

    pub(crate) fn root_entry_record(&self) -> TypeId {
        self.surface.root_entry_record()
    }
}

/// Why a later occurrence of an already-declared Product was refused.
///
/// One Product ledger identity may serve many root occurrences exactly when every
/// occurrence's claim is identical. A divergent occurrence is two different declarations
/// wearing one identity; the draft records the first such conflict and the encoder
/// refuses to emit the image, so a divergent graph can never reach an accepted artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProductClaimConflict {
    /// The occurrence declares a different member/value graph.
    Graph(DurableProductIdentity),
    /// The occurrence declares the same graph with a different entry record.
    EntryRecord(DurableProductIdentity),
}

/// The flat table of durable Product declarations, keyed by Product ledger identity.
#[derive(Debug, Default, Clone)]
pub(crate) struct ProductDeclarationTable {
    rows: Vec<ProductDeclaration>,
    by_identity: BTreeMap<DurableProductIdentity, usize>,
}

impl ProductDeclarationTable {
    /// Admit one root's Product claim, returning the declaration row it references.
    ///
    /// The first occurrence of a Product identity claims the row and binds its graph and
    /// surface; a later occurrence is a reference that reclaims nothing and must match
    /// both exactly. A divergent later occurrence still resolves to the already-bound row
    /// — the encoder refuses the image through the returned conflict — so the caller's
    /// occurrence row still references a real declaration.
    pub(crate) fn admit(
        &mut self,
        claim: DurableProductClaim,
        surface: ProductEntryRecordClaim,
        stamp: RowStamp,
    ) -> (usize, Option<ProductClaimConflict>) {
        let identity = claim.identity();
        let Some(&row) = self.by_identity.get(&identity) else {
            let row = self.rows.len();
            self.by_identity.insert(identity, row);
            self.rows.push(ProductDeclaration {
                claim,
                surface,
                stamp,
            });
            return (row, None);
        };
        let declared = &self.rows[row];
        if declared.claim.graph() != claim.graph() {
            return (row, Some(ProductClaimConflict::Graph(identity)));
        }
        if declared.surface != surface {
            return (row, Some(ProductClaimConflict::EntryRecord(identity)));
        }
        (row, None)
    }

    pub(crate) fn declaration(&self, row: usize) -> &ProductDeclaration {
        &self.rows[row]
    }

    pub(crate) fn row_of(&self, identity: DurableProductIdentity) -> Option<usize> {
        self.by_identity.get(&identity).copied()
    }

    pub(crate) fn declarations(&self) -> &[ProductDeclaration] {
        &self.rows
    }

    /// Drop every declaration appended after `len`, restoring the table to the exact
    /// rows it held at that mark. The table is append-only, so a row's index — the
    /// occurrence rows' reference into it — is stable across the truncation.
    ///
    /// The index is rebuilt by removing exactly the discarded rows' identities, so the
    /// cost is the discarded suffix rather than the whole table: a template proof that
    /// declares nothing must not pay for every declaration the real draft already holds.
    pub(crate) fn truncate(&mut self, len: usize) {
        for discarded in self.rows.drain(len..) {
            self.by_identity.remove(&discarded.claim.identity());
        }
    }
}

/// One flat root-occurrence row: the occurrence facts of one `store` root, referencing
/// the Product declaration it projects.
#[derive(Debug, Clone)]
pub(crate) struct RootOccurrence {
    declaration: usize,
    name: StrId,
    keys: Vec<KeyColumn>,
    placement: RootPlacementIdentity,
    indexes: Vec<DurableIndexShape>,
    stamp: RowStamp,
}

impl RootOccurrence {
    pub(crate) fn declaration(&self) -> usize {
        self.declaration
    }

    pub(crate) fn name(&self) -> StrId {
        self.name
    }

    pub(crate) fn keys(&self) -> &[KeyColumn] {
        &self.keys
    }

    pub(crate) fn placement(&self) -> RootPlacementIdentity {
        self.placement
    }

    pub(crate) fn indexes(&self) -> &[DurableIndexShape] {
        &self.indexes
    }
}

/// The occurrence facts one appended row carries, before the table stamps it.
pub(crate) struct RootOccurrenceRow {
    pub(crate) declaration: usize,
    pub(crate) name: StrId,
    pub(crate) keys: Vec<KeyColumn>,
    pub(crate) placement: RootPlacementIdentity,
    pub(crate) indexes: Vec<DurableIndexShape>,
}

/// The flat root-occurrence table. Each row carries the stamp that keeps it
/// distinguishable from a later row reusing its ordinal.
#[derive(Debug, Default, Clone)]
pub(crate) struct RootOccurrenceTable {
    rows: Vec<RootOccurrence>,
}

impl RootOccurrenceTable {
    /// Append one occurrence row and publish its selector.
    pub(crate) fn push(
        &mut self,
        draft: DraftIdentity,
        row: RootOccurrenceRow,
        stamp: RowStamp,
    ) -> Option<RootOccurrenceSelector> {
        let ordinal = RootOccurrenceOrdinal(u16::try_from(self.rows.len()).ok()?);
        self.rows.push(RootOccurrence {
            declaration: row.declaration,
            name: row.name,
            keys: row.keys,
            placement: row.placement,
            indexes: row.indexes,
            stamp,
        });
        Some(RootOccurrenceSelector {
            draft,
            ordinal,
            stamp,
        })
    }

    pub(crate) fn rows(&self) -> &[RootOccurrence] {
        &self.rows
    }

    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    pub(crate) fn truncate(&mut self, len: usize) {
        self.rows.truncate(len);
    }

    /// The live row `selector` names, or `None` if it was discarded or never belonged to
    /// this table.
    fn live(&self, selector: &RootOccurrenceSelector) -> Option<&RootOccurrence> {
        let row = self.rows.get(selector.ordinal.index())?;
        (row.stamp == selector.stamp).then_some(row)
    }
}

/// A completed root-occurrence row's published selector.
///
/// It names one occurrence without spelling an ordinal a caller could write: it carries
/// the draft it was published by and the exact live row it was published for, and exposes
/// no field, constructor, accessor, `Default`, or `From`. It is `Clone` but deliberately
/// not `Copy` — a selector is a published capability to name an occurrence, and copying
/// one implicitly is how a carrier ends up naming an occurrence it was never given.
///
/// A staged row publishes none; only a completed canonical publication does.
#[derive(Clone)]
pub struct RootOccurrenceSelector {
    draft: DraftIdentity,
    ordinal: RootOccurrenceOrdinal,
    stamp: RowStamp,
}

impl RootOccurrenceSelector {
    /// The DURABLE-table index of the occurrence this selector names — the discriminant
    /// an entry identity `Id(^root)` carries on the wire.
    ///
    /// It is crate-private and published only through [`crate::AdmittedRoot`], so a wire
    /// fact the compiler must emit does not become a way for a consumer to read the
    /// selector's ordinal back out.
    pub(crate) fn wire_root_id(&self) -> u16 {
        self.ordinal.0
    }
}

impl std::fmt::Debug for RootOccurrenceSelector {
    /// One fixed marker. A selector's ordinal and stamp are the authority it carries, so
    /// rendering them would publish in a log what the type exists to keep unforgeable.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("root-occurrence selector")
    }
}

/// Which canonical path a [`CanonicalDeclarationPathSelector`] names. The cases are
/// closed: the root occurrence's own placement, one of that occurrence's managed indexes,
/// or one row of the Product declaration graph. No borrowed path, raw ledger id, source
/// spelling, pointer, or self-reference appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CanonicalDeclarationPathOrdinal {
    /// The root occurrence's own keyed placement.
    RootPlacement,
    /// The managed index at this ordinal of the root occurrence's index list.
    RootIndex(u16),
    /// One row of the Product declaration's member graph.
    DeclarationNode(DeclarationNodeOrdinal),
}

/// The row that published one canonical path selector: the occurrence row for the
/// root-scoped cases, or the Product declaration row for a member case.
#[derive(Clone, Copy)]
enum PathPublisher {
    Occurrence {
        ordinal: RootOccurrenceOrdinal,
        stamp: RowStamp,
    },
    Declaration {
        row: usize,
        stamp: RowStamp,
    },
}

/// A canonical path row's published selector: a root's own placement, one of its managed
/// indexes, or one Product declaration member.
///
/// Product member, group, and branch cases live once in the shared Product declaration
/// rows; the root-whole and root-scoped-index cases live only in their flat occurrence
/// row. Like [`RootOccurrenceSelector`] it is opaque, `Clone` but not `Copy`, carries the
/// exact live row it was published by, and exposes no ordinal.
#[derive(Clone)]
pub struct CanonicalDeclarationPathSelector {
    draft: DraftIdentity,
    publisher: PathPublisher,
    ordinal: CanonicalDeclarationPathOrdinal,
}

impl std::fmt::Debug for CanonicalDeclarationPathSelector {
    /// One fixed marker, for the same reason [`RootOccurrenceSelector`]'s renders one.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("canonical declaration path selector")
    }
}

/// One Product declaration member as the compiler reads it back: the selector that names
/// its canonical path, and its declared shape.
///
/// The shape carries no members — a member's own members are reached by asking the draft
/// for them ([`crate::ImageDraft::members_of`]), so reading a declaration back is
/// navigational and materializes one level at a time.
#[derive(Debug, Clone)]
pub struct DeclarationMember {
    path: CanonicalDeclarationPathSelector,
    shape: DeclarationMemberShape,
}

impl DeclarationMember {
    pub fn path(&self) -> &CanonicalDeclarationPathSelector {
        &self.path
    }

    pub fn shape(&self) -> &DeclarationMemberShape {
        &self.shape
    }
}

/// Why a selector pair could not be bound. The three cases the image owner distinguishes
/// privately; the public [`crate::SitePlanStateError`] projects them as one opaque
/// invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindRefusal {
    /// A selector was published by another draft.
    WrongPlan,
    /// The row a selector was published by is gone, or a later row reused its ordinal.
    StaleBinding,
    /// The path is not a canonical path of this occurrence's Product, or the target does
    /// not apply to the node the path names.
    InvalidDemand,
}

/// The tables a site binding is validated against: the occurrence rows and the Product
/// declarations, plus the draft they belong to.
pub(crate) struct OccurrenceGraph<'draft> {
    pub(crate) draft: DraftIdentity,
    pub(crate) occurrences: &'draft RootOccurrenceTable,
    pub(crate) products: &'draft ProductDeclarationTable,
}

impl<'draft> OccurrenceGraph<'draft> {
    /// Publish the selectors a freshly completed occurrence row owns: the row itself, its
    /// own placement path, and one path per managed index, in index order.
    pub(crate) fn publish(
        &self,
        occurrence: &RootOccurrenceSelector,
    ) -> Option<(
        CanonicalDeclarationPathSelector,
        Vec<CanonicalDeclarationPathSelector>,
    )> {
        let row = self.occurrences.live(occurrence)?;
        let publisher = PathPublisher::Occurrence {
            ordinal: occurrence.ordinal,
            stamp: occurrence.stamp,
        };
        let placement = CanonicalDeclarationPathSelector {
            draft: self.draft,
            publisher,
            ordinal: CanonicalDeclarationPathOrdinal::RootPlacement,
        };
        // The index count is not bounded until `encode` rechecks it, and this entry point
        // exists for callers the compiler does not write, so an unrepresentable ordinal is
        // refused here rather than left to a narrowing panic.
        let indexes = (0..row.indexes.len())
            .map(|index| {
                Some(CanonicalDeclarationPathSelector {
                    draft: self.draft,
                    publisher,
                    ordinal: CanonicalDeclarationPathOrdinal::RootIndex(u16::try_from(index).ok()?),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some((placement, indexes))
    }

    /// The direct members of the Product declaration `identity` names, in declaration
    /// order.
    pub(crate) fn product_members(
        &self,
        identity: DurableProductIdentity,
    ) -> Option<Vec<DeclarationMember>> {
        let row = self.products.row_of(identity)?;
        let declaration = self.products.declarations().get(row)?;
        Some(self.members(row, declaration, declaration.graph().member_ordinals()))
    }

    /// The direct members of the declaration table row `row`, in declaration order.
    ///
    /// The row ordinal is the one [`ProductDeclarationTable::admit`] just returned, so it
    /// names a live declaration by construction and the answer is total: an admitter has
    /// no "no such declaration" case to consider.
    pub(crate) fn members_of_row(&self, row: usize) -> Vec<DeclarationMember> {
        let declaration = self.products.declaration(row);
        self.members(row, declaration, declaration.graph().member_ordinals())
    }

    /// The direct members of the declaration node `path` names, in declaration order. A
    /// root-scoped path and a field declare none.
    pub(crate) fn members_of(
        &self,
        path: &CanonicalDeclarationPathSelector,
    ) -> Option<Vec<DeclarationMember>> {
        if path.draft != self.draft {
            return None;
        }
        let (row, declaration) = self.live_declaration(path)?;
        let CanonicalDeclarationPathOrdinal::DeclarationNode(node) = path.ordinal else {
            return Some(Vec::new());
        };
        Some(self.members(
            row,
            declaration,
            declaration.graph().member_ordinals_of(node),
        ))
    }

    fn members(
        &self,
        row: usize,
        declaration: &'draft ProductDeclaration,
        ordinals: impl Iterator<Item = DeclarationNodeOrdinal>,
    ) -> Vec<DeclarationMember> {
        let publisher = PathPublisher::Declaration {
            row,
            stamp: declaration.stamp,
        };
        ordinals
            .filter_map(|ordinal| {
                let node = declaration.graph().row(ordinal)?;
                Some(DeclarationMember {
                    path: CanonicalDeclarationPathSelector {
                        draft: self.draft,
                        publisher,
                        ordinal: CanonicalDeclarationPathOrdinal::DeclarationNode(ordinal),
                    },
                    shape: node.shape.clone(),
                })
            })
            .collect()
    }

    fn live_declaration(
        &self,
        path: &CanonicalDeclarationPathSelector,
    ) -> Option<(usize, &'draft ProductDeclaration)> {
        let PathPublisher::Declaration { row, stamp } = path.publisher else {
            return None;
        };
        let declaration = self.products.declarations().get(row)?;
        (declaration.stamp == stamp).then_some((row, declaration))
    }

    /// Validate one `(occurrence, path, target)` triple against the live tables and
    /// return the demand key it names.
    ///
    /// This is the whole of what makes a site demand well-formed on the producer side:
    /// both selectors belong to this draft and to live rows, the path is a canonical path
    /// of exactly this occurrence's Product (or exactly this occurrence's own root-scoped
    /// case), and the one supplied target is the target that node admits.
    pub(crate) fn validate(
        &self,
        occurrence: &RootOccurrenceSelector,
        path: &CanonicalDeclarationPathSelector,
        target: crate::semantic::SemanticTarget,
    ) -> Result<BoundDemand, BindRefusal> {
        use crate::semantic::SemanticTarget;

        if occurrence.draft != self.draft || path.draft != self.draft {
            return Err(BindRefusal::WrongPlan);
        }
        let row = self
            .occurrences
            .live(occurrence)
            .ok_or(BindRefusal::StaleBinding)?;

        let publisher_stamp = match path.publisher {
            PathPublisher::Occurrence { ordinal, stamp } => {
                if ordinal != occurrence.ordinal {
                    return Err(BindRefusal::InvalidDemand);
                }
                if stamp != occurrence.stamp {
                    return Err(BindRefusal::StaleBinding);
                }
                stamp
            }
            PathPublisher::Declaration {
                row: declaration,
                stamp,
            } => {
                let declared = self
                    .products
                    .declarations()
                    .get(declaration)
                    .ok_or(BindRefusal::StaleBinding)?;
                if declared.stamp != stamp {
                    return Err(BindRefusal::StaleBinding);
                }
                if declaration != row.declaration {
                    return Err(BindRefusal::InvalidDemand);
                }
                stamp
            }
        };

        let admitted = match path.ordinal {
            CanonicalDeclarationPathOrdinal::RootPlacement => SemanticTarget::WholePayload,
            CanonicalDeclarationPathOrdinal::RootIndex(index) => {
                let shape = row
                    .indexes
                    .get(usize::from(index))
                    .ok_or(BindRefusal::InvalidDemand)?;
                if shape.unique {
                    SemanticTarget::IndexLookup
                } else {
                    SemanticTarget::IndexScan
                }
            }
            CanonicalDeclarationPathOrdinal::DeclarationNode(node) => {
                let declaration = self.products.declarations()[row.declaration].graph();
                match declaration
                    .row(node)
                    .ok_or(BindRefusal::InvalidDemand)?
                    .shape
                {
                    DeclarationMemberShape::Field { .. } => SemanticTarget::FieldLeaf,
                    DeclarationMemberShape::Group { .. } => SemanticTarget::GroupEntry,
                    DeclarationMemberShape::Branch { .. } => SemanticTarget::WholePayload,
                }
            }
        };
        if admitted != target {
            return Err(BindRefusal::InvalidDemand);
        }

        Ok(BoundDemand {
            occurrence: occurrence.ordinal,
            occurrence_stamp: occurrence.stamp,
            path: path.ordinal,
            path_stamp: publisher_stamp,
            target,
        })
    }

    /// Re-prove that the rows one already-validated demand was bound against are still
    /// the rows that are there.
    ///
    /// A binding is proved against the live tables and the handle it produces borrows
    /// nothing, so the rows may have been discarded before the site is requested. The
    /// stamps are what make that observable: an ordinal reused after a discard carries a
    /// fresh stamp, so a handle over a removed row cannot authenticate its replacement.
    pub(crate) fn revalidate(&self, demand: &BoundDemand) -> Result<BoundDemand, BindRefusal> {
        let row = self
            .occurrences
            .rows()
            .get(demand.occurrence.index())
            .ok_or(BindRefusal::StaleBinding)?;
        if row.stamp != demand.occurrence_stamp {
            return Err(BindRefusal::StaleBinding);
        }
        match demand.path {
            CanonicalDeclarationPathOrdinal::RootPlacement
            | CanonicalDeclarationPathOrdinal::RootIndex(_) => {
                if demand.path_stamp != demand.occurrence_stamp {
                    return Err(BindRefusal::StaleBinding);
                }
            }
            CanonicalDeclarationPathOrdinal::DeclarationNode(_) => {
                let declared = self
                    .products
                    .declarations()
                    .get(row.declaration)
                    .ok_or(BindRefusal::StaleBinding)?;
                if declared.stamp != demand.path_stamp {
                    return Err(BindRefusal::StaleBinding);
                }
            }
        }
        Ok(*demand)
    }

    /// Project the semantic path of one validated demand: the chain of kind-tagged ledger
    /// ids from the application down to the node the demand names.
    ///
    /// The plan retains the demand's ordinals, never a path, so this is where a path is
    /// minted — transiently, at encode, by the one path owner. A root's path is
    /// `[Application, Placement]`; an index extends it with the index step; a declaration
    /// member extends it with its ancestry's field, group, and placement steps.
    pub(crate) fn project_path(
        &self,
        application: LedgerIdBytes,
        key: OccurrenceSiteDemandKey,
    ) -> Option<crate::semantic::SemanticPath> {
        use crate::semantic::{SemanticPath, SemanticStep, SemanticStepKind};

        let row = self.occurrences.rows().get(key.occurrence.index())?;
        let root = SemanticPath::root(application, row.placement.ledger_id());
        match key.path {
            CanonicalDeclarationPathOrdinal::RootPlacement => Some(root),
            CanonicalDeclarationPathOrdinal::RootIndex(index) => {
                let shape = row.indexes.get(usize::from(index))?;
                Some(root.child(SemanticStep::new(SemanticStepKind::Index, shape.id)))
            }
            CanonicalDeclarationPathOrdinal::DeclarationNode(node) => {
                let graph = self.products.declarations().get(row.declaration)?.graph();
                let mut path = root;
                for ancestor in graph.ancestry(node) {
                    path = path.child(match &ancestor.shape {
                        DeclarationMemberShape::Field { id, .. } => {
                            SemanticStep::new(SemanticStepKind::Field, *id)
                        }
                        DeclarationMemberShape::Group { id } => {
                            SemanticStep::new(SemanticStepKind::Group, *id)
                        }
                        DeclarationMemberShape::Branch { placement, .. } => {
                            SemanticStep::new(SemanticStepKind::Placement, *placement)
                        }
                    });
                }
                Some(path)
            }
        }
    }
}

/// A validated `(occurrence, canonical path, target)` triple: the complete demand key
/// plus the exact live rows it was validated against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BoundDemand {
    pub(crate) occurrence: RootOccurrenceOrdinal,
    pub(crate) occurrence_stamp: RowStamp,
    pub(crate) path: CanonicalDeclarationPathOrdinal,
    pub(crate) path_stamp: RowStamp,
    pub(crate) target: crate::semantic::SemanticTarget,
}

impl BoundDemand {
    /// The retained key: the three owned typed stable ordinals, and nothing else. The row
    /// stamps are the binding's provenance, not part of what makes two demands the same
    /// place.
    pub(crate) fn key(&self) -> OccurrenceSiteDemandKey {
        OccurrenceSiteDemandKey {
            occurrence: self.occurrence,
            path: self.path,
            target: self.target,
        }
    }
}

/// The one demand a site row answers: which root occurrence, which canonical declaration
/// path, and which operation target over it.
///
/// Every component is an owned typed stable ordinal into the same flat canonical graph,
/// so the key is fixed-width and allocation-free. It is deliberately not the addressed
/// node's ledger id: a Product member declaration below two root occurrences has one
/// declaration id and two distinct places, so a terminal-id key would hand one occurrence
/// the other occurrence's site.
///
/// This key is strictly finer than the `(semantic path, target)` pair the site table
/// deduplicated on before it, so it can only fail to merge two demands that the coarser key
/// merged — never merge two the coarser key kept apart. Where it could differ is where two
/// distinct `(occurrence, node)` pairs project one identical path: two occurrences sharing a
/// placement ledger id, or two declaration rows with equal ancestry and equal ledger ids.
/// Both are one ledger id claimed by two durable declarations, which is refused twice over:
/// the identity ledger refuses to parse two anchors carrying one id, and the independent
/// verifier refuses such an image at its table phase. So the two keys agree on every graph
/// that can be *accepted*, which is what byte preservation is a claim about. That double
/// rejection is the load-bearing fact here, and its verifier half is pinned exhaustively by
/// the pairwise identity matrix over all eleven declaration kinds
/// (`marrow-verify/tests/enum_reuse_hostile.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct OccurrenceSiteDemandKey {
    occurrence: RootOccurrenceOrdinal,
    path: CanonicalDeclarationPathOrdinal,
    target: crate::semantic::SemanticTarget,
}

impl OccurrenceSiteDemandKey {
    pub(crate) fn target(&self) -> crate::semantic::SemanticTarget {
        self.target
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DeclarationCommandError, DeclarationMemberDef, DeclarationMemberShape, KeyColumn,
        LedgerIdBytes, MAX_DURABLE_MEMBERS, ProductDeclarationGraph,
    };
    use crate::draft::{ImageDraft, TypeId};
    use crate::ty::Scalar;
    use crate::value_dag::CanonicalValueShapeDag;

    fn id(byte: u8) -> LedgerIdBytes {
        LedgerIdBytes::from_bytes([byte; 16])
    }

    fn field(
        values: &mut CanonicalValueShapeDag,
        parent: Option<u16>,
        byte: u8,
    ) -> DeclarationMemberDef {
        DeclarationMemberDef {
            parent,
            shape: DeclarationMemberShape::Field {
                id: id(byte),
                required: true,
                value: values.scalar(Scalar::Int),
            },
        }
    }

    /// A resource whose members nest through a group and a branch, stated in pre-order so
    /// the level-order placement has real reordering to do.
    fn nested(values: &mut CanonicalValueShapeDag) -> Vec<DeclarationMemberDef> {
        vec![
            field(values, None, 0x10),
            DeclarationMemberDef {
                parent: None,
                shape: DeclarationMemberShape::Group { id: id(0x20) },
            },
            field(values, Some(1), 0x21),
            field(values, Some(1), 0x22),
            DeclarationMemberDef {
                parent: None,
                shape: DeclarationMemberShape::Branch {
                    placement: id(0x30),
                    name: ImageDraft::new().intern_string("notes"),
                    record: TypeId(7),
                    keys: vec![KeyColumn {
                        scalar: Scalar::Int,
                        id: id(0x31),
                    }],
                },
            },
            field(values, Some(4), 0x32),
            DeclarationMemberDef {
                parent: Some(4),
                shape: DeclarationMemberShape::Group { id: id(0x33) },
            },
            field(values, Some(6), 0x34),
        ]
    }

    /// Every row records its parent and every parent precedes its children, so the
    /// nesting bound is decided by one forward pass rather than a descent.
    #[test]
    fn every_row_is_placed_after_its_parent_and_carries_its_depth() {
        let mut values = CanonicalValueShapeDag::new();
        let graph = ProductDeclarationGraph::from_commands(nested(&mut values))
            .expect("a well-formed forest");

        for (ordinal, node) in graph.rows().iter().enumerate() {
            if let Some(parent) = node.parent {
                assert!(
                    parent.index() < ordinal,
                    "a parent row precedes every row declared under it",
                );
            }
        }
        assert_eq!(
            graph.depths(),
            // The three top-level members, then the group's two fields, then the
            // branch's field and group, then that group's field.
            vec![1, 1, 1, 2, 2, 2, 2, 3],
        );
    }

    /// A node's direct members are one contiguous run of rows, so a walk of the graph is
    /// a slice rather than an owned child vector per node.
    #[test]
    fn a_nodes_members_are_one_contiguous_run() {
        let mut values = CanonicalValueShapeDag::new();
        let graph = ProductDeclarationGraph::from_commands(nested(&mut values))
            .expect("a well-formed forest");

        let members = graph.members();
        assert_eq!(members.len(), 3);
        assert!(matches!(
            members[1].shape(),
            DeclarationMemberShape::Group { .. }
        ));
        assert_eq!(graph.members_of(&members[0]).len(), 0, "a field nests none");
        assert_eq!(graph.members_of(&members[1]).len(), 2);
        let branch_members = graph.members_of(&members[2]);
        assert_eq!(branch_members.len(), 2);
        assert_eq!(graph.members_of(&branch_members[1]).len(), 1);
    }

    /// Two occurrences of one Product identity claim the same graph exactly when their
    /// declarations are the same ordered forest: the placement is injective, so equal
    /// rows are equal declarations and a reordering is a divergence.
    #[test]
    fn equal_rows_are_exactly_equal_declarations() {
        let mut values = CanonicalValueShapeDag::new();
        let graph =
            ProductDeclarationGraph::from_commands(nested(&mut values)).expect("well-formed");

        assert_eq!(
            graph,
            ProductDeclarationGraph::from_commands(nested(&mut values)).expect("well-formed")
        );

        let mut reordered = nested(&mut values);
        reordered.swap(2, 3);
        assert_ne!(
            graph,
            ProductDeclarationGraph::from_commands(reordered).expect("well-formed")
        );

        let mut widened = nested(&mut values);
        widened.push(field(&mut values, None, 0x40));
        assert_ne!(
            graph,
            ProductDeclarationGraph::from_commands(widened).expect("well-formed")
        );
    }

    /// The ancestry of a row is the chain from its top-level member down to it, so a
    /// path projection walks parents rather than descending the graph.
    #[test]
    fn ancestry_is_the_chain_from_the_top_level_member_down() {
        let mut values = CanonicalValueShapeDag::new();
        let graph =
            ProductDeclarationGraph::from_commands(nested(&mut values)).expect("well-formed");

        let deepest = graph.rows().len() - 1;
        let chain = graph.ancestry(super::DeclarationNodeOrdinal(
            u16::try_from(deepest).expect("bounded"),
        ));

        assert_eq!(chain.len(), 3);
        assert!(matches!(
            chain[0].shape(),
            DeclarationMemberShape::Branch { .. }
        ));
        assert!(matches!(
            chain[1].shape(),
            DeclarationMemberShape::Group { .. }
        ));
        assert!(matches!(
            chain[2].shape(),
            DeclarationMemberShape::Field { .. }
        ));
    }

    /// A command vector that does not state a forest is refused rather than materialized:
    /// a parent must be an earlier command, and it must be a node members may nest under.
    #[test]
    fn a_command_vector_that_is_not_a_forest_is_refused() {
        let mut values = CanonicalValueShapeDag::new();
        assert_eq!(
            ProductDeclarationGraph::from_commands(vec![field(&mut values, Some(0), 0x10)]),
            Err(DeclarationCommandError::ParentNotEarlier),
        );
        assert_eq!(
            ProductDeclarationGraph::from_commands(vec![
                field(&mut values, None, 0x10),
                field(&mut values, Some(0), 0x11),
            ]),
            Err(DeclarationCommandError::ParentDeclaresNoMembers),
        );
    }

    /// A declaration past the member bound materializes only the bounded prefix and
    /// records that it did, so the work is bounded by the bound rather than by the input
    /// and the encoder still refuses the declaration through that bound.
    #[test]
    fn a_declaration_past_the_member_bound_materializes_only_the_bounded_prefix() {
        let mut values = CanonicalValueShapeDag::new();
        let wide: Vec<DeclarationMemberDef> = (0..MAX_DURABLE_MEMBERS + 64)
            .map(|n| field(&mut values, None, u8::try_from(n % 251).expect("in range")))
            .collect();

        let graph = ProductDeclarationGraph::from_commands(wide).expect("a well-formed forest");

        assert_eq!(graph.rows().len(), MAX_DURABLE_MEMBERS);
        assert!(graph.over_member_bound());
    }

    /// A fitting declaration records no overflow, so the bound recheck cannot refuse an
    /// image the producer built inside its capacity.
    #[test]
    fn a_fitting_declaration_records_no_overflow() {
        let mut values = CanonicalValueShapeDag::new();
        let graph =
            ProductDeclarationGraph::from_commands(nested(&mut values)).expect("well-formed");

        assert!(!graph.over_member_bound());
        assert_eq!(graph.rows().len(), 8);
    }
}
