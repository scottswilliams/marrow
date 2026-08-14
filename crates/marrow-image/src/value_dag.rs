//! The canonical durable value-shape DAG: the sole representation of a durable
//! field's stored value shape.
//!
//! A durable field's value is drawn from the closed acyclic durable value set — a
//! scalar (a nominal erases to its base scalar), a dense `struct` of positional
//! leaves, or a closed `enum` carrying a sum identity and one member identity per
//! variant. Those shapes *nest*, and nesting is shared: one struct type reached from
//! four fields of four enclosing levels is one declaration, not 256 occurrences.
//!
//! This module is the one place that fact is represented. A [`CanonicalValueShapeDag`]
//! holds each distinct shape once as a node, and every nested position holds a
//! [`ValueShapeNodeId`] reference. Two properties follow structurally:
//!
//! - **It cannot state a cycle.** A node is minted only from ids that already exist,
//!   so every reference points strictly backwards in the arena. Acyclicity is not a
//!   checked property of a submitted graph; it is a property of the only way to build
//!   one.
//! - **It cannot state an occurrence tree.** No node owns a nested node. A caller
//!   holding a `ValueShapeNodeId` has a reference into one arena, and the arena's size
//!   is the number of distinct shapes the program declares.
//!
//! Nodes are interned, so structurally identical shapes share one id: equality of two
//! value shapes is equality of two `u32`s, and the arena is the canonical form.
//!
//! # Depth
//!
//! `MAX_DURABLE_VALUE_DEPTH` bounds how deeply a durable field's value nests. Depth is
//! a property of a *path*, not of a type: a struct used both as a top-level field
//! value and nested twenty levels down is one node at two depths. Deciding the bound
//! from whichever depth a walk happens to see first is wrong in one of two directions —
//! it either refuses a shallow occurrence that fits or admits a deep one the
//! independent verifier will reject.
//!
//! Each node therefore carries [`CanonicalValueShapeDag::depth`]: the length of the
//! longest path from that node down to a scalar, counting the node itself as one
//! level. It is exact and order-independent, because a node's references all point at
//! already-minted nodes whose depth is already final — the interning order *is* a
//! topological order, so one forward pass computes it and there is no second pass to
//! disagree with. A field value rooted at node `n` occupies levels `1..=depth(n)`, so
//! the whole value fits exactly when `depth(n) <= MAX_DURABLE_VALUE_DEPTH`, whatever
//! depth any of `n`'s shared descendants reaches through some other field.
//!
//! # Expansion
//!
//! The v0 wire encodings — the durable contract's identity preimage and the image's
//! DURABLE section — both spell a value shape as a fully expanded tree. Expanding a
//! shared graph is exponential in its nesting depth, so [`expand`] never builds one:
//! it walks an explicit work stack and writes each byte straight into a
//! [`ImageByteSink`], which may stop accepting bytes at a ceiling. A shape whose
//! expansion is larger than any image may be is therefore decided in the bytes the
//! sink admits, not in the bytes the expansion would have produced.
//!
//! A shape can also be too *wide* to spell rather than too large to hold: both forms
//! write an arity as a `u16`, so a node stating more positions than that has no byte
//! image in either. [`expand`] refuses such a shape with [`DurableGraphTooLarge`] before
//! the count reaches the wire, so an arity is never narrowed onto it — two shapes sharing
//! one identity — and never decided by aborting the caller.

use std::collections::HashMap;

use crate::draft::DraftStateError;
use crate::durable_id::{DurableGraphTooLarge, IDREF_MEMBER, IDREF_SUM, LedgerIdBytes};
use crate::ty::Scalar;

/// A reference to one node of a [`CanonicalValueShapeDag`].
///
/// The ordinal is private: an id is obtained only by minting a node into an arena, so
/// it always refers to a node that exists, in the arena that minted it. There is
/// deliberately no way to build one from a raw integer, and no way to build a value
/// shape without an arena.
///
/// It authenticates nothing beyond that. An id carries no arena identity and no
/// generation, so it names whatever node currently sits at its ordinal: held across a
/// [`CanonicalValueShapeDag::truncate`] that drops that node, it would name the next
/// shape minted in its place. Every arena in the tree is either append-only for its whole
/// life or truncated only by an owner that discards the ids it took with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValueShapeNodeId(u32);

/// The live bytes one interned value-shape node occupies in the arena: the node itself,
/// its `depth` word, and the interning entry that finds it again. A dense composite's
/// reference vector is charged separately by the owner that states how many leaves it
/// admits, because a node's own width is not fixed.
///
/// Published for the same reason as the declaration-row charges: an admission owner sizes
/// its maximum-live equation against this representation, so widening a node moves the
/// equation rather than costing silently.
pub(crate) const VALUE_SHAPE_NODE_BYTES: u64 = size_of::<ValueShapeNode>() as u64
    + size_of::<u32>() as u64
    + size_of::<(ValueShapeNode, ValueShapeNodeId)>() as u64;

/// The live bytes one reference inside a value shape occupies, at the widest of the three
/// kinds: a dense composite's leaf id, an enum's member (its ledger identity and the
/// pointer triple of its own payload vector), or one payload leaf id.
pub(crate) const VALUE_SHAPE_REFERENCE_BYTES: u64 = {
    let leaf = size_of::<ValueShapeNodeId>() as u64;
    let member = size_of::<ValueShapeEnumMember>() as u64;
    if member > leaf { member } else { leaf }
};

/// One distinct durable value shape. Nested positions are references, so this type
/// cannot express a tree and cloning it copies one level.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ValueShapeNode {
    Scalar(Scalar),
    /// A dense struct's leaves, positionally. Names are not identity.
    Struct(Vec<ValueShapeNodeId>),
    Enum {
        sum: LedgerIdBytes,
        members: Vec<ValueShapeEnumMember>,
    },
}

/// One variant of a closed enum value: its `Member` ledger identity and its dense
/// payload leaves in declaration order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValueShapeEnumMember {
    id: LedgerIdBytes,
    payload: Vec<ValueShapeNodeId>,
}

impl ValueShapeEnumMember {
    /// This variant's `Member` ledger identity.
    pub fn id(&self) -> LedgerIdBytes {
        self.id
    }

    /// This variant's dense payload leaves, in declaration order.
    pub fn payload(&self) -> &[ValueShapeNodeId] {
        &self.payload
    }
}

/// One node of a [`CanonicalValueShapeDag`], as a reader sees it: the shape's kind and
/// its direct references, never an owned subshape. A caller that needs a nested shape
/// asks the arena for it, so reading a node cannot walk into an expansion by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueShapeView<'a> {
    Scalar(Scalar),
    Struct(&'a [ValueShapeNodeId]),
    Enum {
        sum: LedgerIdBytes,
        members: &'a [ValueShapeEnumMember],
    },
}

/// The program's distinct durable value shapes, each held once.
#[derive(Debug, Clone, Default)]
pub struct CanonicalValueShapeDag {
    store: ValueShapeNodeStore,
    interned: HashMap<ValueShapeNode, ValueShapeNodeId>,
}

impl PartialEq for CanonicalValueShapeDag {
    /// Two arenas are equal when they hold the same nodes in the same order. The
    /// interning map is derived from the nodes, and the depths are derived from the
    /// nodes, so neither participates.
    fn eq(&self, other: &Self) -> bool {
        self.store.same_nodes(&other.store)
    }
}

impl Eq for CanonicalValueShapeDag {}

impl CanonicalValueShapeDag {
    /// An arena holding no shapes.
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of distinct shapes minted. This is the size of the retained value
    /// representation for a whole program, whatever its expanded occurrence count.
    pub fn len(&self) -> usize {
        self.store.len()
    }

    /// Whether this arena holds no shapes.
    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    /// The node for a scalar value (a nominal's caller erases it to its base scalar
    /// first).
    pub fn scalar(&mut self, scalar: Scalar) -> Result<ValueShapeNodeId, DraftStateError> {
        self.intern(ValueShapeNode::Scalar(scalar))
    }

    /// The node for a dense struct value over already-minted leaves, in positional
    /// order.
    ///
    /// A leaf this arena never minted — an id carried over from another arena — is
    /// [`DraftStateError::ForeignDraft`], never an out-of-range index.
    pub fn struct_shape(
        &mut self,
        leaves: Vec<ValueShapeNodeId>,
    ) -> Result<ValueShapeNodeId, DraftStateError> {
        self.intern(ValueShapeNode::Struct(leaves))
    }

    /// The node for a closed enum value: its sum identity and, per variant in
    /// declaration order, its member identity and already-minted payload leaves.
    ///
    /// Checked exactly like [`Self::struct_shape`] over every payload leaf.
    pub fn enum_shape(
        &mut self,
        sum: LedgerIdBytes,
        members: Vec<(LedgerIdBytes, Vec<ValueShapeNodeId>)>,
    ) -> Result<ValueShapeNodeId, DraftStateError> {
        let members = members
            .into_iter()
            .map(|(id, payload)| ValueShapeEnumMember { id, payload })
            .collect();
        self.intern(ValueShapeNode::Enum { sum, members })
    }

    /// The longest path from `node` down to a scalar, counting `node` itself as one
    /// level, or `None` when `node` names nothing here. A top-level durable field value
    /// rooted here occupies exactly that many levels.
    pub fn depth(&self, node: ValueShapeNodeId) -> Option<usize> {
        self.store.depth_of(node).map(|depth| depth as usize)
    }

    /// Whether `node` names a minted node of this arena. An id is minted only against
    /// an arena, but it carries no arena identity, so one minted by a *different* arena
    /// can be out of range here.
    pub fn contains(&self, node: ValueShapeNodeId) -> bool {
        self.store.get(node).is_some()
    }

    /// Read one node: its kind and its direct references, or `None` when `node` names
    /// nothing here. This is the only way to look inside a shape, and it hands back
    /// references rather than subshapes, so no reader can obtain an owned nested tree.
    pub fn view(&self, node: ValueShapeNodeId) -> Option<ValueShapeView<'_>> {
        Some(match self.store.get(node)? {
            ValueShapeNode::Scalar(scalar) => ValueShapeView::Scalar(*scalar),
            ValueShapeNode::Struct(leaves) => ValueShapeView::Struct(leaves),
            ValueShapeNode::Enum { sum, members } => ValueShapeView::Enum { sum: *sum, members },
        })
    }

    /// Every node of this arena, in minting order — which is a topological order, so a
    /// node always follows the nodes it references. A whole-arena bound recheck walks
    /// this once and touches each distinct shape one time.
    pub fn nodes(&self) -> impl Iterator<Item = ValueShapeNodeId> {
        // `intern` refuses a mint that would carry the count past `u32::MAX`, so every
        // reachable length converts; clamping rather than asserting keeps the reader
        // free of a panic that the mint bound already makes unreachable.
        let count = u32::try_from(self.store.len()).unwrap_or(u32::MAX);
        (0..count).map(ValueShapeNodeId)
    }

    /// Drop every node minted at or after `len`, restoring the arena to the state a
    /// caller recorded when it held exactly `len` nodes.
    ///
    /// Minting is append-only and every reference points strictly backwards, so the
    /// retained prefix cannot reference a dropped node. The interning entries for the
    /// dropped nodes go with them: leaving one behind would hand a later caller an id
    /// past the end of the arena.
    pub(crate) fn truncate(&mut self, len: usize) {
        if len >= self.store.len() {
            return;
        }
        self.interned.retain(|_, id| id.index() < len);
        self.store.truncate(len);
    }
    // drop-path audit sentinel: end of CanonicalValueShapeDag::truncate

    /// Mint `node`, or return the id of the structurally identical node already held.
    ///
    /// Every reference `node` carries must already be minted *here*: its depth is then
    /// final, so one `max` over the direct references is the exact longest path and the
    /// arena stays acyclic by construction. An arena is reachable from another crate
    /// through [`crate::product::DurableContractGraph::value_shapes_mut`], so both
    /// premises are checked rather than assumed — an id from another arena and an arena
    /// at its carrier ceiling are the closed builder-domain refusals, and neither
    /// mutates a single owner.
    fn intern(&mut self, node: ValueShapeNode) -> Result<ValueShapeNodeId, DraftStateError> {
        if let Some(existing) = self.interned.get(&node) {
            return Ok(*existing);
        }
        let depth = 1 + self.max_reference_depth(&node)?;
        // Refuse at `u32::MAX` rather than past it: the id minted here is the pre-push
        // length, so admitting that length would leave a post-push count no `u32` holds
        // and `nodes()` unable to name the last node.
        if self.store.len() >= u32::MAX as usize {
            return Err(DraftStateError::CarrierDomain);
        }
        let id = ValueShapeNodeId(
            u32::try_from(self.store.len()).map_err(|_| DraftStateError::CarrierDomain)?,
        );
        self.interned.insert(node.clone(), id);
        self.store.push(node, depth);
        Ok(id)
    }

    /// The greatest depth among `node`'s direct references, or zero when it has none.
    fn max_reference_depth(&self, node: &ValueShapeNode) -> Result<u32, DraftStateError> {
        match node {
            ValueShapeNode::Scalar(_) => Ok(0),
            ValueShapeNode::Struct(leaves) => self.max_depth(leaves),
            ValueShapeNode::Enum { members, .. } => {
                let mut deepest = 0;
                for member in members {
                    deepest = deepest.max(self.max_depth(&member.payload)?);
                }
                Ok(deepest)
            }
        }
    }

    fn max_depth(&self, references: &[ValueShapeNodeId]) -> Result<u32, DraftStateError> {
        let mut deepest = 0;
        for reference in references {
            let depth = self
                .store
                .depth_of(*reference)
                .ok_or(DraftStateError::ForeignDraft)?;
            deepest = deepest.max(depth);
        }
        Ok(deepest)
    }
}

use node_store::ValueShapeNodeStore;

/// The arena's backing store, holding the minted nodes and their final depths behind
/// the only lookup that exists for them.
///
/// The two vectors are private to this module, and a module's ancestors cannot reach a
/// descendant's private items, so no method of [`CanonicalValueShapeDag`] — public or
/// private — can index them on an id it did not mint. A [`ValueShapeNodeId`] carries no
/// arena identity, so one minted elsewhere is in the domain of every accessor that takes
/// one; behind this boundary such an id is a `None` rather than an out-of-range abort
/// reachable through the safe public surface.
mod node_store {
    use super::{ValueShapeNode, ValueShapeNodeId};

    #[derive(Debug, Clone, Default)]
    pub(super) struct ValueShapeNodeStore {
        nodes: Vec<ValueShapeNode>,
        /// The longest path from the node at the same ordinal down to a scalar, counting
        /// that node itself. Written once, when the node is minted, from references whose
        /// own depth is already final.
        depth: Vec<u32>,
    }

    impl ValueShapeNodeStore {
        pub(super) fn len(&self) -> usize {
            self.nodes.len()
        }

        pub(super) fn is_empty(&self) -> bool {
            self.nodes.is_empty()
        }

        /// Whether two stores hold the same nodes in the same order. Depth is derived
        /// from the nodes, so it does not participate.
        pub(super) fn same_nodes(&self, other: &Self) -> bool {
            self.nodes == other.nodes
        }

        pub(super) fn get(&self, node: ValueShapeNodeId) -> Option<&ValueShapeNode> {
            self.nodes.get(node.index())
        }

        pub(super) fn depth_of(&self, node: ValueShapeNodeId) -> Option<u32> {
            self.depth.get(node.index()).copied()
        }

        /// Append one minted node with its final depth. The two vectors move together, so
        /// an ordinal that names a node always names its depth.
        pub(super) fn push(&mut self, node: ValueShapeNode, depth: u32) {
            self.nodes.push(node);
            self.depth.push(depth);
        }

        pub(super) fn truncate(&mut self, len: usize) {
            self.nodes.truncate(len);
            self.depth.truncate(len);
        }
        // drop-path audit sentinel: end of ValueShapeNodeStore::truncate
    }

    #[cfg(test)]
    mod tests {
        use super::ValueShapeNodeStore;

        /// The half of the [`super::super::VALUE_SHAPE_NODE_BYTES`] pricing artifact that
        /// reaches this module's private fields: a vector added beside these two fails to
        /// build until the published per-node charge has been re-derived.
        #[test]
        fn the_priced_store_names_all_of_its_fields() {
            let _ = |value: &ValueShapeNodeStore| {
                let ValueShapeNodeStore { nodes, depth } = value;
                let _ = (nodes, depth);
            };
        }
    }
}

impl ValueShapeNodeId {
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

/// A destination for image bytes: the DURABLE section under construction, or a counter
/// standing in for one.
///
/// The expansion of a shared value shape can be exponentially larger than the shape
/// itself, so a sink may refuse further bytes once it has seen enough to decide its
/// caller's question. [`expand`] stops as soon as a sink reports [`ImageByteSink::is_full`],
/// so the work an expansion costs is the bytes its sink accepts, never the bytes the
/// whole tree would occupy.
pub trait ImageByteSink {
    fn push(&mut self, byte: u8);

    fn extend_bytes(&mut self, bytes: &[u8]);

    /// Whether this sink has all the bytes it needs. Once true it stays true.
    fn is_full(&self) -> bool {
        false
    }
}

impl ImageByteSink for Vec<u8> {
    fn push(&mut self, byte: u8) {
        Vec::push(self, byte);
    }

    fn extend_bytes(&mut self, bytes: &[u8]) {
        self.extend_from_slice(bytes);
    }
}

/// How a value shape's bytes are spelled. The two v0 forms differ only in how a ledger
/// identity is written, so one expansion owner serves both and they cannot drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueShapeWireForm {
    /// The durable contract's canonical identity payload: a ledger id is a kind-tagged
    /// length-prefixed `IDREF`.
    ContractPayload,
    /// The image DURABLE section: a ledger id is its raw 16 bytes.
    DurableSection,
}

/// The frozen value-shape tag bytes. Shared by both wire forms.
const VSHAPE_SCALAR: u8 = 0;
const VSHAPE_STRUCT: u8 = 1;
const VSHAPE_ENUM: u8 = 2;

/// One unit of pending expansion work.
///
/// An enum's variants are written one after another, each with its own header before
/// its payload, so a variant is its own work item rather than a position inside its
/// node's header. A variant task carries the variant itself, borrowed from the arena the
/// expansion walks, so there is no second lookup to get wrong and no arm for a node kind
/// the task could never name. Nothing follows a node's references, so the stack never
/// holds a "resume after children" continuation and its depth is bounded by the shape's
/// own nesting depth.
enum ExpandTask<'a> {
    Node(ValueShapeNodeId),
    EnumMember(&'a ValueShapeEnumMember),
}

/// Write the expanded bytes of the value shape rooted at `root` into `sink`, or refuse a
/// shape stating an arity no v0 wire form can spell.
///
/// The expansion is iterative and direct-to-sink: no expanded tree is built, and no
/// intermediate buffer holds one. It returns as soon as the whole shape is written, the
/// sink reports it is full, or a reference is refused, whichever comes first.
///
/// `root` is a caller-supplied id, and an id names a node only in the arena that minted
/// it, so the lookup is checked: a root from another arena is the same refusal as an
/// arity the wire cannot spell rather than an abort. Nested references are the arena's
/// own, and the mint invariant keeps them in range, so that arm answers for a graph the
/// declaration recheck would already have refused.
pub(crate) fn expand(
    dag: &CanonicalValueShapeDag,
    root: ValueShapeNodeId,
    form: ValueShapeWireForm,
    sink: &mut impl ImageByteSink,
) -> Result<(), DurableGraphTooLarge> {
    let mut tasks = vec![ExpandTask::Node(root)];
    while let Some(task) = tasks.pop() {
        if sink.is_full() {
            return Ok(());
        }
        match task {
            ExpandTask::Node(id) => match dag.store.get(id).ok_or(DurableGraphTooLarge)? {
                ValueShapeNode::Scalar(scalar) => {
                    sink.push(VSHAPE_SCALAR);
                    sink.push(scalar.tag());
                }
                ValueShapeNode::Struct(leaves) => {
                    sink.push(VSHAPE_STRUCT);
                    push_u16(sink, wire_count(leaves.len())?);
                    tasks.extend(leaves.iter().rev().map(|leaf| ExpandTask::Node(*leaf)));
                }
                ValueShapeNode::Enum { sum, members } => {
                    sink.push(VSHAPE_ENUM);
                    push_identity(sink, form, IDREF_SUM, sum);
                    push_u16(sink, wire_count(members.len())?);
                    tasks.extend(members.iter().rev().map(ExpandTask::EnumMember));
                }
            },
            ExpandTask::EnumMember(member) => {
                push_identity(sink, form, IDREF_MEMBER, &member.id);
                push_u16(sink, wire_count(member.payload.len())?);
                tasks.extend(
                    member
                        .payload
                        .iter()
                        .rev()
                        .map(|leaf| ExpandTask::Node(*leaf)),
                );
            }
        }
    }
    Ok(())
}

/// The wire's `u16` count for a value shape's `count` positions, or [`DurableGraphTooLarge`]
/// for an arity no v0 wire form can spell.
///
/// Every arity a durable program states is bounded far below the wire's width —
/// [`crate::bounds::MAX_STRUCT_LEAVES`], [`crate::bounds::MAX_VARIANTS`], and
/// [`crate::bounds::MAX_PAYLOAD_FIELDS`] are all under 256, and the whole arena is
/// rechecked against them before anything is encoded. An arena is public, though, so a
/// caller can state a wider shape and ask the identity owner for its identity directly. A
/// wrapping cast would then give that shape the arity of a narrower one, so two distinct
/// shapes would share one durable-contract identity; refusing keeps the answer typed, and
/// keeps it the identity owner's rather than a precondition on whoever holds the arena.
///
/// A refused shape is genuinely unencodable rather than merely inconvenient: both v0 forms
/// spell an arity as a `u16`, so no image can carry it and no decoder could read it back.
fn wire_count(count: usize) -> Result<u16, DurableGraphTooLarge> {
    u16::try_from(count).map_err(|_| DurableGraphTooLarge)
}

/// Append one big-endian `u16`. The one owner of that spelling for every image sink;
/// callers narrow their own counts, deliberately.
pub(crate) fn push_u16(sink: &mut impl ImageByteSink, value: u16) {
    sink.extend_bytes(&value.to_be_bytes());
}

/// Write one ledger identity in the wire form's spelling: a kind-tagged length-prefixed
/// `IDREF` in the contract payload, raw 16 bytes in the DURABLE section.
fn push_identity(
    sink: &mut impl ImageByteSink,
    form: ValueShapeWireForm,
    kind: u8,
    id: &LedgerIdBytes,
) {
    if form == ValueShapeWireForm::ContractPayload {
        sink.push(kind);
        sink.extend_bytes(&(16u64).to_be_bytes());
    }
    sink.extend_bytes(id.bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger_id(byte: u8) -> LedgerIdBytes {
        LedgerIdBytes::from_bytes([byte; 16])
    }

    /// **The enforcement artifact for [`VALUE_SHAPE_NODE_BYTES`].** The arena and its node
    /// name all of their fields here, so a field added to either fails to build until the
    /// published per-node charge — and the maximum-live equations derived from it — have
    /// been re-derived. A new side table would otherwise cost bytes no charge covers.
    /// The backing store's own two vectors are named by the half of this artifact that
    /// lives beside them, since its fields are private to their module.
    #[test]
    fn the_priced_arena_and_node_name_all_of_their_fields() {
        let _ = |value: &CanonicalValueShapeDag| {
            let CanonicalValueShapeDag { store, interned } = value;
            let _ = (store, interned);
        };
        let _ = |value: &ValueShapeNode| match value {
            ValueShapeNode::Scalar(scalar) => {
                let _ = scalar;
            }
            ValueShapeNode::Struct(leaves) => {
                let _ = leaves;
            }
            ValueShapeNode::Enum { sum, members } => {
                let _ = (sum, members);
            }
        };
        let _ = |value: &ValueShapeEnumMember| {
            let ValueShapeEnumMember { id, payload } = value;
            let _ = (id, payload);
        };
    }

    /// An id minted by one arena names no node in another, and an arena is reachable
    /// from another crate through `DurableContractGraph::value_shapes_mut`. Both
    /// composite minters therefore refuse it as [`DraftStateError::ForeignDraft`] and
    /// leave the target arena byte-for-byte unchanged — the id is never used as an
    /// index into the depth vector, which is what previously aborted the process.
    #[test]
    fn a_foreign_leaf_is_the_typed_refusal_and_mutates_no_node() {
        let mut minting = CanonicalValueShapeDag::new();
        let foreign = minting.scalar(Scalar::Int).expect("the test arena mints");

        let mut empty = CanonicalValueShapeDag::new();
        assert_eq!(
            empty.struct_shape(vec![foreign]),
            Err(DraftStateError::ForeignDraft),
        );
        assert_eq!(
            empty.enum_shape(ledger_id(1), vec![(ledger_id(2), vec![foreign])]),
            Err(DraftStateError::ForeignDraft),
        );
        assert_eq!(empty, CanonicalValueShapeDag::new(), "no node was minted");
        assert_eq!(empty.len(), 0);

        // The same id is not foreign to an arena that has minted that many nodes: the
        // refusal is a range fact about *this* arena, so a populated target accepts it
        // and the two arms above cannot pass by refusing everything.
        let mut populated = CanonicalValueShapeDag::new();
        populated
            .scalar(Scalar::Text)
            .expect("the test arena mints");
        let composite = populated
            .struct_shape(vec![foreign])
            .expect("an in-range leaf is accepted");
        assert_eq!(populated.depth(composite), Some(2));
    }

    /// Every lookup the arena publishes answers a foreign id instead of aborting.
    ///
    /// Refusing the two composite mints is not enough on its own: an arena is reachable
    /// from another crate through `DurableContractGraph::value_shapes_mut`, so a caller
    /// holding an id minted somewhere else reaches `depth`, `view`, and `contains` on the
    /// same handle, and the expansion takes a caller-supplied root as well. Each of them
    /// indexed a backing vector directly, so the abort the mints refuse stayed one method
    /// away. The backing vectors now sit behind a module boundary whose only lookup is
    /// checked, and this pins what that boundary buys at the surface.
    #[test]
    fn every_published_lookup_answers_a_foreign_id_rather_than_aborting() {
        let mut minting = CanonicalValueShapeDag::new();
        let int = minting.scalar(Scalar::Int).expect("the test arena mints");
        let foreign = minting
            .struct_shape(vec![int, int])
            .expect("the test arena mints");

        // The positive arm first: against its own arena every lookup answers, so no arm
        // below can pass by refusing everything.
        assert_eq!(minting.depth(foreign), Some(2));
        assert_eq!(
            minting.view(foreign),
            Some(ValueShapeView::Struct(&[int, int])),
        );
        assert!(minting.contains(foreign));
        let mut written: Vec<u8> = Vec::new();
        expand(
            &minting,
            foreign,
            ValueShapeWireForm::DurableSection,
            &mut written,
        )
        .expect("its own arena expands the shape");
        assert!(!written.is_empty(), "the accepted expansion wrote bytes");

        let empty = CanonicalValueShapeDag::new();
        assert_eq!(empty.depth(foreign), None);
        assert_eq!(empty.view(foreign), None);
        assert!(!empty.contains(foreign));
        let mut refused: Vec<u8> = Vec::new();
        assert_eq!(
            expand(
                &empty,
                foreign,
                ValueShapeWireForm::DurableSection,
                &mut refused,
            ),
            Err(DurableGraphTooLarge),
        );
        assert!(refused.is_empty(), "the refused expansion wrote nothing");
    }

    /// A shape minted twice is one node: the arena is the canonical form, so equality
    /// of shapes is equality of ids.
    #[test]
    fn structurally_identical_shapes_share_one_node() {
        let mut dag = CanonicalValueShapeDag::new();
        let int = dag.scalar(Scalar::Int).expect("the test arena mints");
        let first = dag
            .struct_shape(vec![int, int])
            .expect("the test arena mints");
        let again = dag.scalar(Scalar::Int).expect("the test arena mints");
        let second = dag
            .struct_shape(vec![again, int])
            .expect("the test arena mints");
        assert_eq!(first, second);
        assert_eq!(dag.len(), 2, "one scalar node and one struct node");
    }

    /// Depth is the longest path down to a scalar, counting the node itself.
    #[test]
    fn depth_is_the_longest_path_to_a_scalar() {
        let mut dag = CanonicalValueShapeDag::new();
        let int = dag.scalar(Scalar::Int).expect("the test arena mints");
        assert_eq!(dag.depth(int), Some(1));
        let pair = dag
            .struct_shape(vec![int, int])
            .expect("the test arena mints");
        assert_eq!(dag.depth(pair), Some(2));
        // A struct holding both the scalar and the pair measures the longer branch.
        let mixed = dag
            .struct_shape(vec![int, pair])
            .expect("the test arena mints");
        assert_eq!(dag.depth(mixed), Some(3));
    }

    /// The same node reached at two different depths keeps one depth — its own — so a
    /// shallow field value and a deep one are decided independently and correctly.
    #[test]
    fn a_shared_node_carries_one_depth_whatever_reaches_it() {
        let mut dag = CanonicalValueShapeDag::new();
        let int = dag.scalar(Scalar::Int).expect("the test arena mints");
        let shared = dag.struct_shape(vec![int]).expect("the test arena mints");
        let mut deep = shared;
        for _ in 0..10 {
            deep = dag.struct_shape(vec![deep]).expect("the test arena mints");
        }
        assert_eq!(
            dag.depth(shared),
            Some(2),
            "the shallow field value stays shallow"
        );
        assert_eq!(dag.depth(deep), Some(12));
    }

    /// The interning order is a topological order whichever order the caller mints in,
    /// so building the deep occurrence first and the shallow one second measures both
    /// identically.
    #[test]
    fn depth_does_not_depend_on_minting_order() {
        let mut deep_first = CanonicalValueShapeDag::new();
        let int = deep_first
            .scalar(Scalar::Int)
            .expect("the test arena mints");
        let shared = deep_first
            .struct_shape(vec![int])
            .expect("the test arena mints");
        let mut chain = shared;
        for _ in 0..5 {
            chain = deep_first
                .struct_shape(vec![chain])
                .expect("the test arena mints");
        }
        let deep_first_pair = (deep_first.depth(shared), deep_first.depth(chain));

        let mut shallow_first = CanonicalValueShapeDag::new();
        let int = shallow_first
            .scalar(Scalar::Int)
            .expect("the test arena mints");
        let shared = shallow_first
            .struct_shape(vec![int])
            .expect("the test arena mints");
        let _ = shallow_first.depth(shared);
        let mut chain = shared;
        for _ in 0..5 {
            chain = shallow_first
                .struct_shape(vec![chain])
                .expect("the test arena mints");
        }
        assert_eq!(
            deep_first_pair,
            (shallow_first.depth(shared), shallow_first.depth(chain)),
        );
    }

    /// A shape whose expansion is exponential is still held in nodes linear in its
    /// declared levels.
    #[test]
    fn a_shared_shape_is_stored_once_however_often_it_is_referenced() {
        let mut dag = CanonicalValueShapeDag::new();
        let mut level = dag.scalar(Scalar::Int).expect("the test arena mints");
        for _ in 0..14 {
            level = dag
                .struct_shape(vec![level; 4])
                .expect("the test arena mints");
        }
        assert_eq!(dag.len(), 15, "one scalar plus fourteen struct levels");
        assert_eq!(dag.depth(level), Some(15));
    }

    /// A counting sink that stops at a ceiling.
    struct Ceiling {
        written: usize,
        ceiling: usize,
    }

    impl ImageByteSink for Ceiling {
        fn push(&mut self, _byte: u8) {
            self.written += 1;
        }

        fn extend_bytes(&mut self, bytes: &[u8]) {
            self.written += bytes.len();
        }

        fn is_full(&self) -> bool {
            self.written > self.ceiling
        }
    }

    /// Expanding a shape whose tree has 4^14 leaves costs the bytes the sink accepts,
    /// not the bytes the tree would occupy.
    #[test]
    fn expansion_stops_at_a_full_sink() {
        let mut dag = CanonicalValueShapeDag::new();
        let mut level = dag.scalar(Scalar::Int).expect("the test arena mints");
        for _ in 0..14 {
            level = dag
                .struct_shape(vec![level; 4])
                .expect("the test arena mints");
        }
        let mut sink = Ceiling {
            written: 0,
            ceiling: 4096,
        };
        expand(&dag, level, ValueShapeWireForm::DurableSection, &mut sink)
            .expect("every arity in this shape is one the wire spells");
        assert!(
            sink.written <= 4096 + 8,
            "expansion overran the ceiling by more than one node's header: {}",
            sink.written,
        );
    }

    /// An arity neither wire form can spell ends the expansion with the typed refusal,
    /// rather than aborting the process inside the conversion.
    ///
    /// A `u16` count is the only width both v0 forms have for an arity, so a shape
    /// stating more positions than that has no byte image in either — which is a refusal
    /// its caller can answer for, not a producer defect.
    #[test]
    fn an_arity_the_wire_cannot_spell_refuses_the_expansion() {
        let mut dag = CanonicalValueShapeDag::new();
        let int = dag.scalar(Scalar::Int).expect("the test arena mints");
        let wide = dag
            .struct_shape(vec![int; u16::MAX as usize + 1])
            .expect("the test arena mints");

        let mut bytes = Vec::new();
        assert_eq!(
            expand(&dag, wide, ValueShapeWireForm::DurableSection, &mut bytes),
            Err(DurableGraphTooLarge),
        );
        assert_eq!(
            bytes,
            vec![VSHAPE_STRUCT],
            "the unspellable count is refused rather than narrowed onto the wire",
        );
    }

    /// The two wire forms differ only in how a ledger identity is spelled: the contract
    /// payload tags and length-prefixes it, the DURABLE section writes it raw.
    #[test]
    fn the_two_wire_forms_differ_only_in_identity_spelling() {
        let mut dag = CanonicalValueShapeDag::new();
        let int = dag.scalar(Scalar::Int).expect("the test arena mints");
        let shape = dag
            .enum_shape(ledger_id(1), vec![(ledger_id(2), vec![int])])
            .expect("the test arena mints");

        let mut payload = Vec::new();
        expand(
            &dag,
            shape,
            ValueShapeWireForm::ContractPayload,
            &mut payload,
        )
        .expect("a two-position shape is one the wire spells");
        let mut section = Vec::new();
        expand(
            &dag,
            shape,
            ValueShapeWireForm::DurableSection,
            &mut section,
        )
        .expect("a two-position shape is one the wire spells");

        assert_eq!(payload.len(), section.len() + 2 * 9);
        assert_eq!(payload[0], VSHAPE_ENUM);
        assert_eq!(payload[1], IDREF_SUM);
        assert_eq!(section[0], VSHAPE_ENUM);
        assert_eq!(&section[1..17], ledger_id(1).bytes());
    }

    /// Max-live measurement harness (the editor-capacity term of record; numbers
    /// recorded in `measure.rs`): drive the real [`expand`] over each worst-case
    /// corpus with a ceiling-capped counting sink, and beside it a real
    /// [`Vec<ExpandTask>`] mirroring the three scheduling arms, recording the
    /// worklist's peak length AND capacity — capacity is what lives — plus the
    /// `Vec`-doubling old/new-buffer overlap. Two corpora bound the admitted domain:
    /// the 31-level 64-edge struct chain, and the 31-level enum chain of 256 members
    /// carrying 64 payload references each, the widest scheduling fan-out the §E
    /// bounds admit.
    #[test]
    #[ignore = "measurement harness: run with --ignored and record the printed numbers"]
    fn measure_the_expansion_worklist_peak_for_the_compact_corpora() {
        struct Capped(usize);
        impl ImageByteSink for Capped {
            fn push(&mut self, _byte: u8) {
                self.0 += 1;
            }
            fn extend_bytes(&mut self, bytes: &[u8]) {
                self.0 += bytes.len();
            }
            fn is_full(&self) -> bool {
                self.0 > crate::bounds::MAX_IMAGE_BYTES
            }
        }

        let struct_chain = |dag: &mut CanonicalValueShapeDag| {
            let mut level = dag.scalar(Scalar::Int).expect("the test arena mints");
            for _ in 0..31 {
                level = dag
                    .struct_shape(vec![level; 64])
                    .expect("the test arena mints");
            }
            level
        };
        let enum_chain = |dag: &mut CanonicalValueShapeDag| {
            let mut level = dag.scalar(Scalar::Int).expect("the test arena mints");
            for depth in 0..31u16 {
                let members = (0..256u16)
                    .map(|member| {
                        let mut bytes = [0u8; 16];
                        bytes[0..2].copy_from_slice(&depth.to_be_bytes());
                        bytes[2..4].copy_from_slice(&member.to_be_bytes());
                        (LedgerIdBytes::from_bytes(bytes), vec![level; 64])
                    })
                    .collect();
                let mut sum = [0xffu8; 16];
                sum[0..2].copy_from_slice(&depth.to_be_bytes());
                level = dag
                    .enum_shape(LedgerIdBytes::from_bytes(sum), members)
                    .expect("the test arena mints");
            }
            level
        };

        for (name, build) in [
            (
                "struct 31x64",
                &struct_chain as &dyn Fn(&mut CanonicalValueShapeDag) -> _,
            ),
            ("enum 31x256x64", &enum_chain),
        ] {
            let mut dag = CanonicalValueShapeDag::new();
            let root = build(&mut dag);

            // The real traversal against the capped sink: the run the term charges.
            let mut sink = Capped(0);
            expand(&dag, root, ValueShapeWireForm::DurableSection, &mut sink)
                .expect("the capped run stops early");

            // The scheduling mirror: a real Vec of the real task type, driven by the
            // same three arms, against the same capped byte budget.
            let mut tasks: Vec<ExpandTask> = vec![ExpandTask::Node(root)];
            let mut bytes = 0usize;
            let mut peak_len = tasks.len();
            let mut peak_capacity = tasks.capacity();
            while let Some(task) = tasks.pop() {
                if bytes > crate::bounds::MAX_IMAGE_BYTES {
                    break;
                }
                match task {
                    ExpandTask::Node(id) => match dag.store.get(id).expect("a minted node") {
                        ValueShapeNode::Scalar(_) => bytes += 2,
                        ValueShapeNode::Struct(leaves) => {
                            bytes += 3;
                            tasks.extend(leaves.iter().rev().map(|leaf| ExpandTask::Node(*leaf)));
                        }
                        ValueShapeNode::Enum { members, .. } => {
                            bytes += 1 + 16 + 2;
                            tasks.extend(members.iter().rev().map(ExpandTask::EnumMember));
                        }
                    },
                    ExpandTask::EnumMember(member) => {
                        bytes += 16 + 2;
                        tasks.extend(
                            member
                                .payload
                                .iter()
                                .rev()
                                .map(|leaf| ExpandTask::Node(*leaf)),
                        );
                    }
                }
                peak_len = peak_len.max(tasks.len());
                peak_capacity = peak_capacity.max(tasks.capacity());
            }
            let task_bytes = size_of::<ExpandTask>();
            println!(
                "{name}: capped run counted {} bytes; worklist peak {peak_len} tasks, \
                 capacity {peak_capacity} x {task_bytes} bytes/task = {} live bytes, \
                 doubling overlap <= {} bytes",
                sink.0,
                peak_capacity * task_bytes,
                (peak_capacity + peak_capacity / 2) * task_bytes,
            );
        }
    }
}
