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
use std::rc::Rc;

use crate::bounds::MAX_DURABLE_MEMBERS;
use crate::draft::{
    AdmittedGraphInputPlan, DraftIdentity, KeyColumn, RootOccurrenceDef, StrId, TypeId,
};
use crate::durable_id::{
    DurableContractView, DurableIndexShape, DurableMemberViews, DurableProductIdentity,
    LedgerIdBytes, RootPlacementIdentity,
};
use crate::value_dag::{CanonicalValueShapeDag, ValueShapeNodeId};

/// The live bytes one flat member command occupies in the vector a caller hands
/// [`crate::ImageDraft::declare_product`].
///
/// Published so the admission owners that size their maximum-live equations charge this
/// representation rather than a sampled fixture: widening a command row moves every
/// equation that consumes it, which is a build failure where the equation is asserted, not
/// a silent cost.
pub(crate) const DECLARATION_COMMAND_BYTES: u64 = size_of::<DeclarationMemberDef>() as u64;

/// The live bytes one materialized member row occupies in a Product's flat member graph.
pub(crate) const DECLARATION_ROW_BYTES: u64 = size_of::<DeclarationNode>() as u64;

/// The live bytes one Product declaration row occupies beside its member rows: the row
/// itself plus the identity index entry that finds it. The index entry is charged with a
/// `BTreeMap` node's own key/value pair and its link word, which is the shape of the
/// per-entry cost a balanced node amortizes.
pub(crate) const PRODUCT_DECLARATION_ROW_BYTES: u64 = size_of::<ProductDeclaration>() as u64
    + size_of::<(DurableProductIdentity, usize)>() as u64
    + size_of::<usize>() as u64;

/// The live bytes one root-occurrence row occupies: the row itself plus its full key
/// tuple, which is a heap vector the row owns and `size_of` cannot see, plus the shared
/// index list's `Rc` control block — sixteen bytes of strong/weak counts that exist per
/// list even when the occurrence declares no index. The occurrence's managed indexes are
/// charged per index by [`MANAGED_INDEX_BYTES`], so an occurrence declaring none is not
/// charged for thirty-two.
pub(crate) const ROOT_OCCURRENCE_ROW_BYTES: u64 = size_of::<RootOccurrence>() as u64
    + crate::bounds::MAX_KEY_COLUMNS as u64 * size_of::<KeyColumn>() as u64
    + 2 * size_of::<usize>() as u64;

/// The live bytes one managed-index declaration occupies apart from its projection.
pub(crate) const MANAGED_INDEX_SHAPE_BYTES: u64 = size_of::<DurableIndexShape>() as u64;

/// The live bytes one projected index component occupies.
pub(crate) const MANAGED_INDEX_COMPONENT_BYTES: u64 =
    size_of::<crate::durable_id::DurableIndexComponent>() as u64;

/// The live bytes one managed-index declaration occupies: the shape itself plus its full
/// leaf projection.
pub(crate) const MANAGED_INDEX_BYTES: u64 = MANAGED_INDEX_SHAPE_BYTES
    + crate::bounds::MAX_INDEX_COMPONENTS as u64 * MANAGED_INDEX_COMPONENT_BYTES;

/// The live bytes an empty durable contract graph occupies before it holds a row: the
/// application identity, the two empty tables, the empty arena, and the draft identity and
/// stamp counter that keep its published selectors distinguishable.
pub(crate) const CONTRACT_GRAPH_FIXED_BYTES: u64 = size_of::<DurableContractGraph>() as u64;

/// The stamp one appended row carries.
///
/// Row ordinals are reused deterministically — a proof pass appends rows and the rewind
/// discards them, and the next append lands on the same ordinal — so an ordinal alone
/// cannot say whether the row a selector, handle, or operand was minted against is still
/// the row that is there. The stamp is drawn from the graph's one monotone counter, which
/// a rewind deliberately does **not** restore, so a re-minted row is never mistaken for
/// the row it replaced.
///
/// It lives beside that counter and its field is private, so
/// [`DurableContractGraph::next_stamp`] is the only place in the crate a stamp comes into
/// being: an owner appending a row against this graph draws one, and no owner can mint a
/// stamp of its own that would collide with a row's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RowStamp(u64);

impl RowStamp {
    /// The stamp for one draw of the graph's monotone counter.
    fn from_counter(count: u64) -> Self {
        Self(count)
    }
}

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

/// A flat member command vector this graph declines to materialize.
///
/// Every command must name a parent that an earlier command declared, and that parent must
/// be a node members may nest under — a static `group` or a keyed `branch`. A field
/// declares no members, so naming one as a parent is not a deep graph but a malformed
/// command vector. A command nesting past [`bounds::MAX_DURABLE_DEPTH`] is well-formed but
/// unrepresentable, and is refused for its own reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeclarationCommandError {
    /// The parent index is not strictly less than the command's own index.
    ParentNotEarlier,
    /// The named parent declares no members.
    ParentDeclaresNoMembers,
    /// The command's own nesting level is past [`bounds::MAX_DURABLE_DEPTH`].
    TooDeep,
}

/// One Product declaration's canonical member/value graph, as flat rows.
///
/// The rows are the declaration: the wire projection, the durable contract payload,
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
    ///
    /// **Nesting depth is decided here, before a row exists.** A parent always precedes
    /// its children in the command vector, so one forward pass fixes every command's level
    /// and a command past [`bounds::MAX_DURABLE_DEPTH`] is refused with nothing
    /// materialized. The bound is a property of the *level*, not of what the level holds:
    /// an over-deep body declaring no member of its own is still an over-deep row, and is
    /// refused as one. Every consumer of these rows — the member walks, the projected site
    /// paths, the contract-identity payload — therefore meets a graph no deeper than a
    /// [`crate::SemanticPath`] can name.
    pub(crate) fn from_commands(
        commands: Vec<DeclarationMemberDef>,
    ) -> Result<Self, DeclarationCommandError> {
        // Group the commands by the parent they name, checking as we go that each parent
        // is an earlier command that may hold members at all and that the command's own
        // level is representable. A command vector wider than the member bound is not
        // malformed — it is a declaration the encoder refuses, so it is admitted here as a
        // bounded prefix that records the overflow.
        let mut children: Vec<Vec<u16>> = vec![Vec::new(); commands.len().min(MAX_DURABLE_MEMBERS)];
        let mut roots: Vec<u16> = Vec::new();
        let mut levels: Vec<usize> = Vec::with_capacity(children.len());
        let over_member_bound = commands.len() > MAX_DURABLE_MEMBERS;
        for (index, command) in commands.iter().enumerate().take(MAX_DURABLE_MEMBERS) {
            let index = u16::try_from(index).expect("the member bound is below u16::MAX");
            let Some(parent) = command.parent else {
                roots.push(index);
                levels.push(1);
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
            let level = levels[usize::from(parent)] + 1;
            if level > crate::bounds::MAX_DURABLE_DEPTH {
                return Err(DeclarationCommandError::TooDeep);
            }
            levels.push(level);
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

    /// One row's nesting depth — the length of its parent chain, 1 for a top-level
    /// member — walked through the parent links without materializing the chain. An
    /// ordinal outside the rows has no chain and depth 0.
    ///
    /// A parent always precedes its children, so the chain strictly descends in row
    /// ordinal and the walk terminates; its length is bounded by
    /// [`crate::bounds::MAX_DURABLE_DEPTH`], enforced by [`Self::from_commands`] before a
    /// row exists.
    fn chain_depth(&self, ordinal: DeclarationNodeOrdinal) -> usize {
        let mut depth = 0;
        let mut cursor = Some(ordinal);
        while let Some(at) = cursor {
            let Some(node) = self.rows.get(at.index()) else {
                break;
            };
            depth += 1;
            cursor = node.parent;
        }
        depth
    }

    /// Visit the chain of rows from a top-level member down to `ordinal`, outermost
    /// first, materializing nothing. An ordinal outside the rows has no chain and
    /// visits nothing.
    ///
    /// Each level is reached by re-walking the parent links from `ordinal`, so the walk
    /// costs at most `depth * depth` link hops over a chain [`Self::chain_depth`] already
    /// bounds — a handful of loads — and allocates no buffer to reverse.
    fn walk_ancestry<'a>(
        &'a self,
        ordinal: DeclarationNodeOrdinal,
        mut visit: impl FnMut(&'a DeclarationNode),
    ) {
        let depth = self.chain_depth(ordinal);
        for level in (0..depth).rev() {
            let mut at = ordinal;
            for _ in 0..level {
                at = self.rows[at.index()]
                    .parent
                    .expect("the chain the depth walk counted has a parent at every hop");
            }
            visit(&self.rows[at.index()]);
        }
    }

    /// Each row's nesting depth, 1 for a top-level member.
    ///
    /// A parent always precedes its children, so one forward pass over the parent
    /// ordinals fixes every depth; no descent of the graph is needed to read them. Every
    /// depth here is within [`crate::bounds::MAX_DURABLE_DEPTH`], because
    /// [`Self::from_commands`] refused the command vector otherwise.
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
    graph: DurableProductGraph,
}

impl DurableProductClaim {
    pub(crate) fn new(identity: DurableProductIdentity, graph: DurableProductGraph) -> Self {
        Self { identity, graph }
    }

    pub(crate) fn identity(&self) -> DurableProductIdentity {
        self.identity
    }

    pub(crate) fn graph(&self) -> &DurableProductGraph {
        &self.graph
    }
}

/// One Product declaration's canonical member graph, held as a shared owner.
///
/// It is the published handle onto rows an admitted declaration already built: it has no
/// public constructor, no public field, and no mutator, so a caller reads a graph a
/// producer declared and can state none of its own. Its members are reached as borrowed
/// views ([`DurableMemberViews`]), which carry no member vector, so this type cannot be
/// turned back into a recursive tree.
///
/// The rows are shared rather than copied: every root occurrence over one Product refers
/// to this one owner, so a Product's member graph is stored once however many occurrences
/// project it, on both the compiler's and the verifier's side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableProductGraph {
    rows: Rc<ProductDeclarationGraph>,
}

impl DurableProductGraph {
    /// The Product's direct members, in declaration order.
    pub fn iter(&self) -> DurableMemberViews<'_> {
        DurableMemberViews::over(&self.rows, self.rows.members())
    }

    pub(crate) fn rows(&self) -> &ProductDeclarationGraph {
        &self.rows
    }
}

impl<'a> IntoIterator for &'a DurableProductGraph {
    type Item = crate::durable_id::DurableMemberView<'a>;
    type IntoIter = DurableMemberViews<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Why a flat durable-graph command was refused before any row was appended.
///
/// Every cause is a fault in the input a caller built — a command vector that states no
/// forest, a count the admitted plan does not cover, or an occurrence of a Product this
/// graph does not hold. None of them is a source diagnostic or a user resource kind: the
/// compiler's own entry points project them into their one opaque refusal, and the
/// independent verifier projects them into its own typed hostile-image rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableGraphInputRefusal {
    /// The command vector is wider, or the declaration/occurrence count higher, than the
    /// admitted input plan covers.
    OverPlan,
    /// The command vector does not state a forest: a member names a parent that is not an
    /// earlier command, or names one that declares no members.
    MalformedCommands,
    /// The command vector nests past [`crate::bounds::MAX_DURABLE_DEPTH`]. A node deeper
    /// than that has no [`crate::SemanticPath`] that can name it, so the declaration is
    /// refused where the rows would be made rather than after they exist.
    OverDepth,
    /// A later occurrence of an already-declared Product states a different member graph.
    DivergentGraph,
    /// A later occurrence of an already-declared Product states a different entry record.
    DivergentEntryRecord,
    /// The occurrence names a Product this graph holds no declaration for.
    UndeclaredProduct,
    /// The occurrence table cannot address another row.
    UnaddressableOccurrence,
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
        self.claim.graph().rows()
    }

    /// This declaration's canonical member graph as the shared owner a caller retains.
    pub(crate) fn product_graph(&self) -> &DurableProductGraph {
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

    /// Admit one Product declaration under `plan`: check the admitted counts, validate the
    /// command vector, and bind or reference the row.
    ///
    /// This is the one place the plan's Product and command budgets are spent, so the
    /// draft and the independent verifier's own contract graph enter construction through
    /// the same checks rather than each restating them. A repeat of an already-bound
    /// identity resolves to that row and declares no new Product, so it spends no Product
    /// budget.
    pub(crate) fn admit_under(
        &mut self,
        plan: &AdmittedGraphInputPlan,
        identity: DurableProductIdentity,
        surface: ProductEntryRecordClaim,
        members: Vec<DeclarationMemberDef>,
        stamp: RowStamp,
    ) -> Result<(usize, Option<ProductClaimConflict>), DurableGraphInputRefusal> {
        if members.len() > plan.commands() {
            return Err(DurableGraphInputRefusal::OverPlan);
        }
        if self.row_of(identity).is_none() && self.declarations().len() >= plan.products() {
            return Err(DurableGraphInputRefusal::OverPlan);
        }
        let graph =
            ProductDeclarationGraph::from_commands(members).map_err(|refusal| match refusal {
                DeclarationCommandError::TooDeep => DurableGraphInputRefusal::OverDepth,
                DeclarationCommandError::ParentNotEarlier
                | DeclarationCommandError::ParentDeclaresNoMembers => {
                    DurableGraphInputRefusal::MalformedCommands
                }
            })?;
        let claim = DurableProductClaim::new(
            identity,
            DurableProductGraph {
                rows: Rc::new(graph),
            },
        );
        Ok(self.admit(claim, surface, stamp))
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
    indexes: Rc<[DurableIndexShape]>,
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
    pub(crate) indexes: Rc<[DurableIndexShape]>,
}

/// The flat root-occurrence table. Each row carries the stamp that keeps it
/// distinguishable from a later row reusing its ordinal.
#[derive(Debug, Default, Clone)]
pub(crate) struct RootOccurrenceTable {
    rows: Vec<RootOccurrence>,
}

impl RootOccurrenceTable {
    /// Append one occurrence row under `plan` and publish its selector.
    ///
    /// This is the one place the plan's root-occurrence budget is spent, so the draft and
    /// the independent verifier's own contract graph enter the table through the same
    /// check rather than each restating it — the same way
    /// [`ProductDeclarationTable::admit_under`] owns the Product and command terms. All
    /// three of the plan's terms are therefore spent by the table that holds the rows they
    /// bound, in one refusal vocabulary.
    pub(crate) fn push_under(
        &mut self,
        plan: &AdmittedGraphInputPlan,
        draft: DraftIdentity,
        row: RootOccurrenceRow,
        stamp: RowStamp,
    ) -> Result<RootOccurrenceSelector, DurableGraphInputRefusal> {
        if self.rows.len() >= plan.roots() {
            return Err(DurableGraphInputRefusal::OverPlan);
        }
        let ordinal = RootOccurrenceOrdinal(
            u16::try_from(self.rows.len())
                .map_err(|_| DurableGraphInputRefusal::UnaddressableOccurrence)?,
        );
        self.rows.push(RootOccurrence {
            declaration: row.declaration,
            name: row.name,
            keys: row.keys,
            placement: row.placement,
            indexes: row.indexes,
            stamp,
        });
        Ok(RootOccurrenceSelector {
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

    /// Stream the semantic-path steps of one validated demand, outermost first: the
    /// application step, the root placement step, then — per the demand's kind — the
    /// index step or the named declaration node's ancestry steps.
    ///
    /// This is the one projection grammar. [`Self::project_path`] collects an owned path
    /// from exactly this stream, so a step's kind, id, and position cannot differ
    /// between a streamed walk and a materialized path. `None` means the demand names
    /// rows that are not there; steps already emitted for such a demand are the caller's
    /// to discard with the refusal.
    pub(crate) fn project_steps(
        &self,
        application: LedgerIdBytes,
        key: OccurrenceSiteDemandKey,
        mut emit: impl FnMut(crate::semantic::SemanticStep),
    ) -> Option<()> {
        use crate::semantic::{SemanticStep, SemanticStepKind};

        let row = self.occurrences.rows().get(key.occurrence.index())?;
        emit(SemanticStep::new(
            SemanticStepKind::Application,
            application,
        ));
        emit(SemanticStep::new(
            SemanticStepKind::Placement,
            row.placement.ledger_id(),
        ));
        match key.path {
            CanonicalDeclarationPathOrdinal::RootPlacement => Some(()),
            CanonicalDeclarationPathOrdinal::RootIndex(index) => {
                let shape = row.indexes.get(usize::from(index))?;
                emit(SemanticStep::new(SemanticStepKind::Index, shape.id));
                Some(())
            }
            CanonicalDeclarationPathOrdinal::DeclarationNode(node) => {
                let graph = self.products.declarations().get(row.declaration)?.graph();
                graph.walk_ancestry(node, |ancestor| {
                    emit(match &ancestor.shape {
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
                });
                Some(())
            }
        }
    }

    /// Project the semantic path of one validated demand: the chain of kind-tagged ledger
    /// ids from the application down to the node the demand names.
    ///
    /// The plan retains the demand's ordinals, never a path, so this is where a path is
    /// minted — transiently, at encode, by the one path owner. The path is collected
    /// from [`Self::project_steps`], the one streaming projection grammar.
    pub(crate) fn project_path(
        &self,
        application: LedgerIdBytes,
        key: OccurrenceSiteDemandKey,
    ) -> Option<crate::semantic::SemanticPath> {
        use crate::semantic::{SemanticPath, SemanticStep};

        let mut application_step: Option<SemanticStep> = None;
        let mut path: Option<SemanticPath> = None;
        self.project_steps(application, key, |step| {
            if let Some(existing) = path.take() {
                path = Some(existing.extend(step));
            } else if let Some(app) = application_step {
                path = Some(SemanticPath::root(app.id, step.id));
            } else {
                application_step = Some(step);
            }
        })?;
        path
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

/// The append-only lengths of every owner a [`DurableContractGraph`] holds at one instant,
/// plus its application-identity slot.
///
/// It is the durable half of the mark a throwaway pass rewinds to. It is crate-private and
/// carries no graph identity, so it is only ever handed back to the graph that published
/// it — the owner that publishes it also owns the exclusive borrow through which a pass
/// appends.
pub(crate) struct DurableGraphCheckpoint {
    application: Option<LedgerIdBytes>,
    products: usize,
    occurrences: usize,
    values: usize,
}

/// One program's durable contract graph, owned.
///
/// It holds the four owners a contract is derived from — the application identity, the
/// canonical Product declaration table, the flat root-occurrence table, and the one
/// value-shape DAG their fields reference — and publishes them as one zero-allocation
/// [`DurableContractView`]. It is the owner the **independent verifier** builds from
/// received bytes; the compiler's draft holds the same four owners beside its own site
/// plan and publishes the same view from them, so both sides compute one canonical
/// contract encoding from rows of one shape rather than from two models that could drift.
///
/// Construction is flat, bottom-up, and admitted: every entry point requires an
/// [`AdmittedGraphInputPlan`], a Product's members arrive as a command vector whose rows
/// name their parent by an earlier command, and no entry accepts a built recursive owner
/// by value. There is no public raw-row constructor and no way to reach the rows except
/// through the borrowed views, so nothing a caller states can be deeper than the rows a
/// bounded producer wrote.
///
/// It owns its arena. A [`crate::ValueShapeNodeId`] a caller reads from this graph's
/// views addresses a node this graph holds, for as long as it holds it.
#[derive(Debug)]
pub struct DurableContractGraph {
    /// The strong identity the occurrence table's published selectors carry.
    identity: DraftIdentity,
    next_stamp: u64,
    application: Option<LedgerIdBytes>,
    products: ProductDeclarationTable,
    occurrences: RootOccurrenceTable,
    values: CanonicalValueShapeDag,
}

impl Default for DurableContractGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl DurableContractGraph {
    /// The empty graph: no application identity, no declaration, no occurrence, and an
    /// empty arena. Its contract identity is the well-defined identity of a program with
    /// no durable data.
    pub fn new() -> Self {
        Self {
            identity: DraftIdentity::mint(),
            next_stamp: 0,
            application: None,
            products: ProductDeclarationTable::default(),
            occurrences: RootOccurrenceTable::default(),
            values: CanonicalValueShapeDag::new(),
        }
    }

    /// The stamp the next appended row carries.
    ///
    /// It is the graph's one stamp source, drawn by every owner that appends a row against
    /// it — including the draft's site plan, whose rows name these rows. A rewind
    /// deliberately does not restore it, so an ordinal reused after a discard carries a
    /// fresh stamp.
    pub(crate) fn next_stamp(&mut self) -> RowStamp {
        self.next_stamp += 1;
        RowStamp::from_counter(self.next_stamp)
    }

    /// The strong identity every selector, handle, and site operand minted against this
    /// graph carries.
    pub(crate) fn identity(&self) -> DraftIdentity {
        self.identity
    }

    /// Record the application's ledger id. A graph with a root always has one.
    pub fn set_application_identity(&mut self, id: LedgerIdBytes) {
        self.application = Some(id);
    }

    /// This graph's application ledger id, if one was recorded.
    pub(crate) fn application(&self) -> Option<LedgerIdBytes> {
        self.application
    }

    /// The admitted Product declarations, each retained once however many roots occur over
    /// it.
    pub(crate) fn products(&self) -> &ProductDeclarationTable {
        &self.products
    }

    /// The flat root-occurrence rows, in declaration order.
    pub(crate) fn occurrences(&self) -> &RootOccurrenceTable {
        &self.occurrences
    }

    /// The live tables a site binding is validated and projected against.
    pub(crate) fn occurrence_graph(&self) -> OccurrenceGraph<'_> {
        OccurrenceGraph {
            draft: self.identity,
            occurrences: &self.occurrences,
            products: &self.products,
        }
    }

    /// The append-only lengths of every owner this graph holds, for a caller that appends
    /// a throwaway pass against it and discards what the pass wrote.
    ///
    /// The graph is destructured exhaustively rather than read field by field: a new owner
    /// stops this compiling until it is either recorded here or bound to `_` beside the two
    /// whose exclusion is stated below, so no field can be left out of a rewind by
    /// omission. `identity` is fixed at mint and no pass can change it; `next_stamp`
    /// advances monotonically on purpose, so a re-minted row is never mistaken for the row
    /// it replaced.
    pub(crate) fn checkpoint(&self) -> DurableGraphCheckpoint {
        let Self {
            identity: _,
            next_stamp: _,
            application,
            products,
            occurrences,
            values,
        } = self;
        DurableGraphCheckpoint {
            application: *application,
            products: products.declarations().len(),
            occurrences: occurrences.len(),
            values: values.len(),
        }
    }

    /// Discard every row appended after `at`, restoring the exact rows the graph held at
    /// that mark. It allocates nothing and cannot panic, so it is safe to run during an
    /// unwind.
    pub(crate) fn rewind(&mut self, at: &DurableGraphCheckpoint) {
        self.occurrences.truncate(at.occurrences);
        self.products.truncate(at.products);
        self.values.truncate(at.values);
        self.application = at.application;
    }

    /// Admit one Product declaration under `plan`, returning the declaration row it
    /// references and the first divergence a repeat of an already-bound identity states.
    ///
    /// The two published entry points differ only in what they do with a divergence: this
    /// graph's own refuses it, while the draft latches it and lets the encoder report it in
    /// wire order. They admit through one path, so neither can admit what the other would
    /// not.
    pub(crate) fn admit_product(
        &mut self,
        plan: &AdmittedGraphInputPlan,
        product: LedgerIdBytes,
        entry_record: TypeId,
        members: Vec<DeclarationMemberDef>,
    ) -> Result<(usize, Option<ProductClaimConflict>), DurableGraphInputRefusal> {
        let stamp = self.next_stamp();
        self.products.admit_under(
            plan,
            DurableProductIdentity::minted(product),
            ProductEntryRecordClaim::new(entry_record),
            members,
            stamp,
        )
    }

    /// Append one root occurrence under `plan` over the Product declaration `product`
    /// names, returning the completed row's selector.
    pub(crate) fn admit_root_occurrence(
        &mut self,
        plan: &AdmittedGraphInputPlan,
        product: LedgerIdBytes,
        def: RootOccurrenceDef,
    ) -> Result<RootOccurrenceSelector, DurableGraphInputRefusal> {
        let declaration = self
            .products
            .row_of(DurableProductIdentity::minted(product))
            .ok_or(DurableGraphInputRefusal::UndeclaredProduct)?;
        let RootOccurrenceDef {
            name,
            keys,
            placement,
            indexes,
        } = def;
        let stamp = self.next_stamp();
        self.occurrences.push_under(
            plan,
            self.identity,
            RootOccurrenceRow {
                declaration,
                name,
                keys,
                placement: RootPlacementIdentity::minted(placement),
                indexes,
            },
            stamp,
        )
    }

    /// This graph's one value-shape arena, for interning the shapes its fields reference.
    pub fn value_shapes_mut(&mut self) -> &mut CanonicalValueShapeDag {
        &mut self.values
    }

    /// This graph's one value-shape arena.
    pub fn value_shapes(&self) -> &CanonicalValueShapeDag {
        &self.values
    }

    /// Admit one durable Product declaration under `plan`, returning the shared owner of
    /// its canonical member graph.
    ///
    /// The first declaration of a Product identity binds the row; a later one is a
    /// reference that must state the identical member graph and entry record, and states
    /// a divergence rather than silently canonicalizing one of them away.
    pub fn declare_product(
        &mut self,
        plan: &AdmittedGraphInputPlan,
        product: LedgerIdBytes,
        entry_record: TypeId,
        members: Vec<DeclarationMemberDef>,
    ) -> Result<DurableProductGraph, DurableGraphInputRefusal> {
        let (row, conflict) = self.admit_product(plan, product, entry_record, members)?;
        match conflict {
            Some(ProductClaimConflict::Graph(_)) => Err(DurableGraphInputRefusal::DivergentGraph),
            Some(ProductClaimConflict::EntryRecord(_)) => {
                Err(DurableGraphInputRefusal::DivergentEntryRecord)
            }
            None => Ok(self.products.declaration(row).product_graph().clone()),
        }
    }

    /// Append one root occurrence over the Product declaration `product` names.
    ///
    /// The row retains only the root's own placement, spelling, key tuple, and managed
    /// indexes and a reference to the one declaration, so nothing is retained per
    /// (root x member).
    pub fn add_root_occurrence(
        &mut self,
        plan: &AdmittedGraphInputPlan,
        product: LedgerIdBytes,
        def: RootOccurrenceDef,
    ) -> Result<(), DurableGraphInputRefusal> {
        self.admit_root_occurrence(plan, product, def).map(|_| ())
    }

    /// This graph's durable contract, borrowed in place.
    pub fn contract_view(&self) -> DurableContractView<'_> {
        DurableContractView::over(
            self.application,
            &self.products,
            &self.occurrences,
            &self.values,
        )
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

    /// **The enforcement artifact for the published row charges.** Every row the
    /// maximum-live equations price names all of its fields here, so adding one fails to
    /// build until the charge that prices it has been re-derived.
    ///
    /// `size_of` sees a heap-owning field as one pointer triple, so a new `Vec` field would
    /// otherwise widen the real cost while leaving every published charge and every
    /// equation over them unchanged and still green. The patterns are typechecked and never
    /// run; a closure body is the smallest place to write one without constructing a value
    /// of every row.
    #[test]
    fn every_priced_row_names_all_of_its_fields() {
        let _ = |value: &super::DeclarationMemberDef| {
            let DeclarationMemberDef { parent, shape } = value;
            let _ = (parent, shape);
        };
        let _ = |value: &super::DeclarationNode| {
            let super::DeclarationNode {
                parent,
                members,
                shape,
            } = value;
            let _ = (parent, members, shape);
        };
        let _ = |value: &super::ProductDeclaration| {
            let super::ProductDeclaration {
                claim,
                surface,
                stamp,
            } = value;
            let _ = (claim, surface, stamp);
        };
        let _ = |value: &super::RootOccurrence| {
            let super::RootOccurrence {
                declaration,
                name,
                keys,
                placement,
                indexes,
                stamp,
            } = value;
            let _ = (declaration, name, keys, placement, indexes, stamp);
        };
        let _ = |value: &super::DurableContractGraph| {
            let super::DurableContractGraph {
                identity,
                next_stamp,
                application,
                products,
                occurrences,
                values,
            } = value;
            let _ = (
                identity,
                next_stamp,
                application,
                products,
                occurrences,
                values,
            );
        };
        // The two tables and the arena the graph holds are charged per row by the equation
        // above, so their own field sets are named too: a table that grew a second index
        // would cost bytes no per-row charge covers.
        let _ = |value: &super::ProductDeclarationTable| {
            let super::ProductDeclarationTable { rows, by_identity } = value;
            let _ = (rows, by_identity);
        };
        let _ = |value: &super::RootOccurrenceTable| {
            let super::RootOccurrenceTable { rows } = value;
            let _ = rows;
        };
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
    /// path projection walks parents rather than descending the graph. The walk streams
    /// the chain outermost first and materializes nothing.
    #[test]
    fn ancestry_is_the_chain_from_the_top_level_member_down() {
        let mut values = CanonicalValueShapeDag::new();
        let graph =
            ProductDeclarationGraph::from_commands(nested(&mut values)).expect("well-formed");

        let deepest = graph.rows().len() - 1;
        let mut chain = Vec::new();
        graph.walk_ancestry(
            super::DeclarationNodeOrdinal(u16::try_from(deepest).expect("bounded")),
            |node| chain.push(node),
        );

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

    /// The allocation-free parent-chain depth agrees with the forward depth pass on
    /// every row, and an ordinal outside the rows has no chain.
    #[test]
    fn the_parent_chain_depth_agrees_with_the_forward_depth_pass() {
        let mut values = CanonicalValueShapeDag::new();
        let graph =
            ProductDeclarationGraph::from_commands(nested(&mut values)).expect("well-formed");

        for (ordinal, depth) in graph.depths().into_iter().enumerate() {
            assert_eq!(
                graph.chain_depth(super::DeclarationNodeOrdinal(
                    u16::try_from(ordinal).expect("bounded"),
                )),
                depth,
                "row {ordinal} carries one depth however it is walked",
            );
        }
        assert_eq!(
            graph.chain_depth(super::DeclarationNodeOrdinal(
                u16::try_from(graph.rows().len()).expect("bounded"),
            )),
            0,
            "an ordinal outside the rows has no chain",
        );
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
