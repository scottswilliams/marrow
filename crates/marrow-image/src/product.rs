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

use std::collections::{BTreeMap, VecDeque};

use crate::bounds::MAX_DURABLE_MEMBERS;
use crate::draft::{DurableMemberDef, KeyColumn, StrId, TypeId};
use crate::durable_id::{
    DurableIndexShape, DurableProductIdentity, DurableValueShape, LedgerIdBytes,
    RootPlacementIdentity,
};

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
/// are rows of their own, reached through [`DeclarationNode::members`] — the row carries
/// no owned member vector, so the graph has exactly one nesting owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeclarationNodeKind {
    Field {
        id: LedgerIdBytes,
        required: bool,
        value: DurableValueShape,
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

/// One row of a Product declaration's flat member graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeclarationNode {
    parent: Option<DeclarationNodeOrdinal>,
    members: DeclarationSpan,
    kind: DeclarationNodeKind,
}

impl DeclarationNode {
    pub(crate) fn kind(&self) -> &DeclarationNodeKind {
        &self.kind
    }
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
    /// The Product's own direct members. Always the leading run, since the flattening
    /// appends them first.
    members: DeclarationSpan,
    /// The declaration declared more members than [`MAX_DURABLE_MEMBERS`], so the rows
    /// are the bounded prefix that was materialized rather than the whole declaration.
    ///
    /// The flattening refuses to materialize an unbounded table for a graph the encoder
    /// will refuse anyway; the encoder reads this and reports the exceeded bound, so a
    /// truncated graph can never be encoded.
    over_member_bound: bool,
}

impl ProductDeclarationGraph {
    /// Flatten one declaration's member tree into rows.
    ///
    /// Members are appended level by level: a node's own members are appended together,
    /// so they occupy one contiguous run, and every parent precedes its children. The
    /// flattening is deterministic and injective over ordered trees, so two occurrences
    /// of one Product identity claim the same graph exactly when their rows are equal.
    pub(crate) fn flatten(members: Vec<DurableMemberDef>) -> Self {
        let mut graph = Self {
            rows: Vec::new(),
            members: DeclarationSpan::default(),
            over_member_bound: false,
        };
        let mut pending: VecDeque<(DeclarationNodeOrdinal, Vec<DurableMemberDef>)> =
            VecDeque::new();
        graph.members = graph.append_level(None, members, &mut pending);
        while let Some((parent, children)) = pending.pop_front() {
            let span = graph.append_level(Some(parent), children, &mut pending);
            graph.rows[parent.index()].members = span;
        }
        graph
    }

    /// Append one node's direct members as a contiguous run, queueing each group's and
    /// branch's own members for the next level. Returns the run.
    fn append_level(
        &mut self,
        parent: Option<DeclarationNodeOrdinal>,
        members: Vec<DurableMemberDef>,
        pending: &mut VecDeque<(DeclarationNodeOrdinal, Vec<DurableMemberDef>)>,
    ) -> DeclarationSpan {
        let Ok(start) = u16::try_from(self.rows.len()) else {
            self.over_member_bound = true;
            return DeclarationSpan::default();
        };
        let mut len = 0u16;
        for member in members {
            if self.rows.len() >= MAX_DURABLE_MEMBERS {
                self.over_member_bound = true;
                break;
            }
            let Ok(ordinal) = u16::try_from(self.rows.len()).map(DeclarationNodeOrdinal) else {
                self.over_member_bound = true;
                break;
            };
            let kind = match member {
                DurableMemberDef::Field {
                    id,
                    required,
                    value,
                } => DeclarationNodeKind::Field {
                    id,
                    required,
                    value,
                },
                DurableMemberDef::Group { id, members } => {
                    pending.push_back((ordinal, members));
                    DeclarationNodeKind::Group { id }
                }
                DurableMemberDef::Branch {
                    placement,
                    name,
                    record,
                    keys,
                    members,
                } => {
                    pending.push_back((ordinal, members));
                    DeclarationNodeKind::Branch {
                        placement,
                        name,
                        record,
                        keys,
                    }
                }
            };
            self.rows.push(DeclarationNode {
                parent,
                members: DeclarationSpan::default(),
                kind,
            });
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

    /// Every row, in the order they were appended: a parent always precedes its children.
    pub(crate) fn rows(&self) -> &[DeclarationNode] {
        &self.rows
    }

    /// The declaration exceeded [`MAX_DURABLE_MEMBERS`] and its rows are a bounded
    /// prefix.
    pub(crate) fn over_member_bound(&self) -> bool {
        self.over_member_bound
    }

    /// Project the rows back into the member tree the compiler still builds and reads.
    ///
    /// The rows are the owner; this is the inverse of [`Self::flatten`], retained only
    /// while the compiler's own graph builder and the draft's construction entry point
    /// still speak in trees. It is deleted with [`DurableMemberDef`].
    pub(crate) fn member_tree(&self) -> Vec<DurableMemberDef> {
        self.subtree(self.members)
    }

    fn subtree(&self, span: DeclarationSpan) -> Vec<DurableMemberDef> {
        self.rows[span.range()]
            .iter()
            .map(|node| match &node.kind {
                DeclarationNodeKind::Field {
                    id,
                    required,
                    value,
                } => DurableMemberDef::Field {
                    id: *id,
                    required: *required,
                    value: value.clone(),
                },
                DeclarationNodeKind::Group { id } => DurableMemberDef::Group {
                    id: *id,
                    members: self.subtree(node.members),
                },
                DeclarationNodeKind::Branch {
                    placement,
                    name,
                    record,
                    keys,
                } => DurableMemberDef::Branch {
                    placement: *placement,
                    name: *name,
                    record: *record,
                    keys: keys.clone(),
                    members: self.subtree(node.members),
                },
            })
            .collect()
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
/// The graph is the whole recursive member tree — every field id, `required` flag, and
/// value shape, every static `group`, and every nested keyed `branch` placement together
/// with its key tuple and its own members. Those are Product facts: they are declared by
/// the resource, not by the root that projects it. The *outer* root's placement identity,
/// spelling, key tuple, and managed indexes are occurrence facts and are deliberately not
/// in the claim, so two roots may share a Product while carrying different placements,
/// key tuples, and index shapes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DurableProductClaim {
    identity: DurableProductIdentity,
    graph: ProductDeclarationGraph,
}

impl DurableProductClaim {
    pub(crate) fn new(identity: DurableProductIdentity, members: Vec<DurableMemberDef>) -> Self {
        Self {
            identity,
            graph: ProductDeclarationGraph::flatten(members),
        }
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
/// — are carried by the declaration's own member graph ([`DeclarationNodeKind::Branch`]),
/// which is already part of [`DurableProductClaim`] and compared with it. They are not
/// copied here: the root entry record is the one such fact the member graph does not
/// carry, because it belongs to the resource rather than to any one member.
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

/// One admitted Product declaration: its claim and the runtime surface bound with it.
#[derive(Debug, Clone)]
pub(crate) struct ProductDeclaration {
    claim: DurableProductClaim,
    surface: ProductEntryRecordClaim,
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
    /// both exactly.
    pub(crate) fn admit(
        &mut self,
        claim: DurableProductClaim,
        surface: ProductEntryRecordClaim,
    ) -> Result<usize, ProductClaimConflict> {
        let identity = claim.identity();
        let Some(&row) = self.by_identity.get(&identity) else {
            let row = self.rows.len();
            self.by_identity.insert(identity, row);
            self.rows.push(ProductDeclaration { claim, surface });
            return Ok(row);
        };
        let declared = &self.rows[row];
        if declared.claim.graph() != claim.graph() {
            return Err(ProductClaimConflict::Graph(identity));
        }
        if declared.surface != surface {
            return Err(ProductClaimConflict::EntryRecord(identity));
        }
        Ok(row)
    }

    pub(crate) fn declaration(&self, row: usize) -> &ProductDeclaration {
        &self.rows[row]
    }

    pub(crate) fn by_identity(
        &self,
        identity: DurableProductIdentity,
    ) -> Option<&ProductDeclaration> {
        self.by_identity.get(&identity).map(|row| &self.rows[*row])
    }

    pub(crate) fn declarations(&self) -> &[ProductDeclaration] {
        &self.rows
    }

    /// Drop every declaration appended after `len`, restoring the table to the exact
    /// rows it held at that mark. The table is append-only, so a row's index — the
    /// occurrence rows' reference into it — is stable across the truncation.
    pub(crate) fn truncate(&mut self, len: usize) {
        self.rows.truncate(len);
        self.by_identity.retain(|_, row| *row < len);
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
}

impl RootOccurrence {
    pub(crate) fn new(
        declaration: usize,
        name: StrId,
        keys: Vec<KeyColumn>,
        placement: RootPlacementIdentity,
        indexes: Vec<DurableIndexShape>,
    ) -> Self {
        Self {
            declaration,
            name,
            keys,
            placement,
            indexes,
        }
    }

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

#[cfg(test)]
mod tests {
    use super::{
        DeclarationNodeKind, DurableMemberDef, KeyColumn, LedgerIdBytes, MAX_DURABLE_MEMBERS,
        ProductDeclarationGraph,
    };
    use crate::draft::{ImageDraft, TypeId};
    use crate::durable_id::DurableValueShape;
    use crate::ty::Scalar;

    fn id(byte: u8) -> LedgerIdBytes {
        LedgerIdBytes::from_bytes([byte; 16])
    }

    fn field(byte: u8) -> DurableMemberDef {
        DurableMemberDef::Field {
            id: id(byte),
            required: true,
            value: DurableValueShape::Scalar(Scalar::Int),
        }
    }

    /// A resource whose members nest through a group and a branch, so the flattening has
    /// more than one level to place.
    fn nested() -> Vec<DurableMemberDef> {
        vec![
            field(0x10),
            DurableMemberDef::Group {
                id: id(0x20),
                members: vec![field(0x21), field(0x22)],
            },
            DurableMemberDef::Branch {
                placement: id(0x30),
                name: ImageDraft::new().intern_string("notes"),
                record: TypeId(7),
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: id(0x31),
                }],
                members: vec![
                    field(0x32),
                    DurableMemberDef::Group {
                        id: id(0x33),
                        members: vec![field(0x34)],
                    },
                ],
            },
        ]
    }

    /// The rows are the declaration: projecting them back reproduces the exact ordered
    /// tree they were flattened from, so nothing the wire or the contract id reads is
    /// lost by owning the graph flat.
    #[test]
    fn flattening_a_declaration_preserves_its_exact_ordered_tree() {
        let members = nested();

        let graph = ProductDeclarationGraph::flatten(members.clone());

        assert_eq!(graph.member_tree(), members);
    }

    /// Every row records its parent and every parent precedes its children, so the
    /// nesting bound is decided by one forward pass rather than a descent.
    #[test]
    fn every_row_is_appended_after_its_parent_and_carries_its_depth() {
        let graph = ProductDeclarationGraph::flatten(nested());

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
        let graph = ProductDeclarationGraph::flatten(nested());

        let members = graph.members();
        assert_eq!(members.len(), 3);
        assert!(matches!(
            members[1].kind(),
            DeclarationNodeKind::Group { .. }
        ));
        assert_eq!(graph.members_of(&members[0]).len(), 0, "a field nests none");
        assert_eq!(graph.members_of(&members[1]).len(), 2);
        let branch_members = graph.members_of(&members[2]);
        assert_eq!(branch_members.len(), 2);
        assert_eq!(graph.members_of(&branch_members[1]).len(), 1);
    }

    /// Two occurrences of one Product identity claim the same graph exactly when their
    /// declarations are the same ordered tree: the flattening is injective, so equal rows
    /// are equal declarations and a reordering is a divergence.
    #[test]
    fn equal_rows_are_exactly_equal_declarations() {
        let graph = ProductDeclarationGraph::flatten(nested());

        assert_eq!(graph, ProductDeclarationGraph::flatten(nested()));

        let mut reordered = nested();
        reordered.swap(0, 1);
        assert_ne!(graph, ProductDeclarationGraph::flatten(reordered));

        let mut renested = nested();
        renested.push(field(0x40));
        assert_ne!(graph, ProductDeclarationGraph::flatten(renested));
    }

    /// The flattening refuses to materialize an unbounded row table for a declaration the
    /// encoder will refuse anyway: it stops at the bound and records that it did, so the
    /// work is bounded by the bound rather than by the input.
    #[test]
    fn a_declaration_past_the_member_bound_materializes_only_the_bounded_prefix() {
        let members: Vec<DurableMemberDef> = (0..MAX_DURABLE_MEMBERS + 64)
            .map(|n| DurableMemberDef::Field {
                id: LedgerIdBytes::from_bytes([u8::try_from(n % 251).expect("in range"); 16]),
                required: false,
                value: DurableValueShape::Scalar(Scalar::Int),
            })
            .collect();

        let graph = ProductDeclarationGraph::flatten(members);

        assert_eq!(graph.rows().len(), MAX_DURABLE_MEMBERS);
        assert!(graph.over_member_bound());
    }

    /// A fitting declaration records no overflow, so the bound recheck cannot refuse an
    /// image the producer built inside its capacity.
    #[test]
    fn a_fitting_declaration_records_no_overflow() {
        let graph = ProductDeclarationGraph::flatten(nested());

        assert!(!graph.over_member_bound());
        assert_eq!(graph.rows().len(), 8);
    }
}
