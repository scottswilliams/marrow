//! The canonical durable value-shape DAG: the sole representation of a durable field's
//! stored value shape.
//!
//! A [`CanonicalValueShapeDag`] holds each distinct shape — a scalar, a dense `struct` of
//! named leaves, or a closed `enum` with a sum identity and, per variant, one member
//! identity and its named payload leaves — once, as an interned node, and every nested
//! position holds a [`ValueShapeNodeId`] carrying the node's arena-local ordinal plus an
//! exact-node stamp. A node is minted only from ids that already exist, so every
//! reference points strictly backwards and the arena cannot state a cycle; no node owns a
//! nested node, so it cannot state an occurrence tree.
//!
//! A stored struct or enum payload value occupies one cell, and its leaves are encoded by
//! position. Declarations are identified by ledger ids; a positional stored leaf is
//! identified by its declared name in declaration order. The name therefore belongs to the
//! shape: two structs with the same leaf types but different leaf names or order are
//! different shapes, so an edit that would read an existing cell's bytes with a different
//! meaning always changes the durable contract.
//!
//! Depth is a property of a path, so each node carries the longest path from itself down
//! to a scalar, and a field value rooted at `n` fits exactly when
//! `depth(n) <= MAX_DURABLE_VALUE_DEPTH`. Both wire forms spell a shape fully expanded,
//! which is exponential in nesting depth, so [`expand`] never builds the tree: it streams
//! bytes into an [`ImageByteSink`] that may stop at a ceiling, and refuses an arity the
//! wire's `u16` cannot spell with [`DurableGraphTooLarge`] rather than narrowing it.
use std::collections::HashMap;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::draft::DraftStateError;
use crate::durable_id::{DurableGraphTooLarge, IDREF_MEMBER, IDREF_SUM, LedgerIdBytes};
use crate::ty::Scalar;

/// A reference to one node of a [`CanonicalValueShapeDag`].
///
/// Both fields are private: an id is obtained only by minting a node into an arena. The
/// process-unique stamp authenticates the exact node at the ordinal, so a same-ordinal id
/// from an independently minted arena and an id held across
/// [`CanonicalValueShapeDag::truncate`] are both stale rather than silently binding to
/// another shape. Cloning an arena preserves its exact nodes and stamps, so its existing
/// ids deliberately remain valid in the clone. There is no way to build one from raw
/// integers, and no way to build a value shape without an arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValueShapeNodeId {
    ordinal: u32,
    stamp: ValueShapeNodeStamp,
}

/// The exact-node provenance carried by a [`ValueShapeNodeId`]. Stamps are never rewound:
/// truncating and reusing an ordinal therefore cannot revive an old id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ValueShapeNodeStamp(u64);

impl ValueShapeNodeStamp {
    fn mint() -> Result<Self, DraftStateError> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self::mint_from(&NEXT)
    }

    fn mint_from(next: &AtomicU64) -> Result<Self, DraftStateError> {
        next.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
            next.checked_add(1)
        })
        .map(Self)
        .map_err(|_| DraftStateError::CarrierDomain)
    }
}

/// The randomized structural hash used only to narrow an interning lookup. Equality is
/// always rechecked against the sole stored node, so a collision cannot merge shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ValueShapeFingerprint(u64);

/// One collision-chain link stored beside a canonical node. `u32::MAX` is the empty
/// sentinel: minting refuses before that ordinal, so no live node can be hidden by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ValueShapeFingerprintPredecessor(u32);

impl ValueShapeFingerprintPredecessor {
    const NONE: u32 = u32::MAX;

    fn from_id(id: Option<ValueShapeNodeId>) -> Self {
        Self(id.map_or(Self::NONE, |id| id.ordinal))
    }

    fn ordinal(self) -> Option<u32> {
        (self.0 != Self::NONE).then_some(self.0)
    }
}

/// One distinct durable value shape. Nested positions are references, so this type
/// cannot express a tree and cloning it copies one level.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ValueShapeNode {
    Scalar(Scalar),
    /// A dense struct's leaves in declaration order, each identified by its declared name.
    Struct(Vec<ValueShapeLeaf>),
    Enum {
        sum: LedgerIdBytes,
        members: Vec<ValueShapeEnumMember>,
    },
}

/// One positional leaf of a stored struct or enum payload: its declared name and its
/// shape. The arena mints a leaf only with a non-empty name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValueShapeLeaf {
    name: Box<str>,
    shape: ValueShapeNodeId,
}

impl ValueShapeLeaf {
    /// A leaf with its declared name and an already-minted shape. The arena refuses an
    /// empty name when the leaf is minted.
    pub fn new(name: impl Into<Box<str>>, shape: ValueShapeNodeId) -> Self {
        Self {
            name: name.into(),
            shape,
        }
    }

    /// The leaf's declared name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The leaf's value shape.
    pub fn shape(&self) -> ValueShapeNodeId {
        self.shape
    }
}

/// One variant of a closed enum value: its `Member` ledger identity and its dense
/// payload leaves in declaration order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValueShapeEnumMember {
    id: LedgerIdBytes,
    payload: Vec<ValueShapeLeaf>,
}

impl ValueShapeEnumMember {
    /// A variant with its `Member` ledger identity and already-minted payload leaves in
    /// declaration order. The arena refuses an unnamed leaf when the enum is minted.
    pub fn new(id: LedgerIdBytes, payload: Vec<ValueShapeLeaf>) -> Self {
        Self { id, payload }
    }

    /// This variant's `Member` ledger identity.
    pub fn id(&self) -> LedgerIdBytes {
        self.id
    }

    /// This variant's dense payload leaves, in declaration order.
    pub fn payload(&self) -> &[ValueShapeLeaf] {
        &self.payload
    }
}

/// One node of a [`CanonicalValueShapeDag`], as a reader sees it: the shape's kind and
/// its direct references, never an owned subshape. A caller that needs a nested shape
/// asks the arena for it, so reading a node cannot walk into an expansion by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueShapeView<'a> {
    Scalar(Scalar),
    Struct(&'a [ValueShapeLeaf]),
    Enum {
        sum: LedgerIdBytes,
        members: &'a [ValueShapeEnumMember],
    },
}

/// The program's distinct durable value shapes, each held once.
#[derive(Debug, Clone, Default)]
pub struct CanonicalValueShapeDag {
    store: ValueShapeNodeStore,
    interned: HashMap<ValueShapeFingerprint, ValueShapeNodeId>,
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

/// Exact comparison of field values from two image graphs. Scratch is reused across
/// fields; the work allowance covers one old image's fully expanded value occurrences.
pub struct ValueShapeComparison<'a, 'b> {
    old: &'a CanonicalValueShapeDag,
    new: &'b CanonicalValueShapeDag,
    frames: Vec<ValuePairFrame<'a, 'b>>,
    remaining: usize,
}

enum ValuePairFrame<'a, 'b> {
    Struct {
        old: &'a [ValueShapeLeaf],
        new: &'b [ValueShapeLeaf],
        next: usize,
    },
    Enum {
        old: &'a [ValueShapeEnumMember],
        new: &'b [ValueShapeEnumMember],
        member: usize,
        next: usize,
    },
}

impl<'a, 'b> ValuePairFrame<'a, 'b> {
    /// The next pair of leaves at the same position. Arities were compared when the frame
    /// was pushed, so the new side always has the position the old side has.
    fn next(&mut self) -> Option<(&'a ValueShapeLeaf, &'b ValueShapeLeaf)> {
        match self {
            Self::Struct { old, new, next } => {
                let pair = (old.get(*next)?, &new[*next]);
                *next += 1;
                Some(pair)
            }
            Self::Enum {
                old,
                new,
                member,
                next,
            } => {
                while let Some(variant) = old.get(*member) {
                    if let Some(leaf) = variant.payload.get(*next) {
                        let pair = (leaf, &new[*member].payload[*next]);
                        *next += 1;
                        return Some(pair);
                    }
                    *member += 1;
                    *next = 0;
                }
                None
            }
        }
    }
}

impl<'a, 'b> ValueShapeComparison<'a, 'b> {
    /// Compare exact scalar, named-leaf struct, or identified enum representation: every
    /// struct and payload leaf must carry the same declared name at the same position.
    /// `None` means foreign IDs, excessive depth, or exhausted image-work allowance.
    ///
    /// Each preserved field occurrence must be paired once by the caller. Verified
    /// DURABLE wire spells every nested value occurrence in full: each visited node and
    /// enum-member header consumes at least one distinct old wire byte, and each leaf
    /// pair is charged its old name's bytes plus one for its marker. Thus one image's
    /// byte cap bounds their aggregate work, including shared DAG revisits. The guard
    /// also refuses arbitrary constructed DAGs whose expansion cannot fit.
    pub fn same(&mut self, old: ValueShapeNodeId, new: ValueShapeNodeId) -> Option<bool> {
        self.frames.clear();
        if self.old.depth(old)? > crate::bounds::MAX_DURABLE_VALUE_DEPTH
            || self.new.depth(new)? > crate::bounds::MAX_DURABLE_VALUE_DEPTH
        {
            return None;
        }
        let mut pair = (old, new);
        loop {
            self.remaining = self.remaining.checked_sub(1)?;
            match (self.old.view(pair.0)?, self.new.view(pair.1)?) {
                (ValueShapeView::Scalar(old), ValueShapeView::Scalar(new)) => {
                    if old != new {
                        return Some(false);
                    }
                }
                (ValueShapeView::Struct(old), ValueShapeView::Struct(new)) => {
                    if old.len() != new.len() {
                        return Some(false);
                    }
                    self.frames
                        .push(ValuePairFrame::Struct { old, new, next: 0 });
                }
                (
                    ValueShapeView::Enum {
                        sum: old_sum,
                        members: old,
                    },
                    ValueShapeView::Enum {
                        sum: new_sum,
                        members: new,
                    },
                ) => {
                    if old_sum != new_sum || old.len() != new.len() {
                        return Some(false);
                    }
                    for (old, new) in old.iter().zip(new) {
                        self.remaining = self.remaining.checked_sub(1)?;
                        if old.id != new.id || old.payload.len() != new.payload.len() {
                            return Some(false);
                        }
                    }
                    self.frames.push(ValuePairFrame::Enum {
                        old,
                        new,
                        member: 0,
                        next: 0,
                    });
                }
                _ => return Some(false),
            }
            loop {
                let Some(frame) = self.frames.last_mut() else {
                    return Some(true);
                };
                if let Some((old, new)) = frame.next() {
                    self.remaining = self.remaining.checked_sub(1 + old.name.len())?;
                    if old.name != new.name {
                        return Some(false);
                    }
                    pair = (old.shape, new.shape);
                    break;
                }
                self.frames.pop();
            }
        }
    }
}

impl CanonicalValueShapeDag {
    /// An arena holding no shapes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Compare this old image's field occurrences against `new`, sharing one bounded
    /// scratch allocation. The two arenas and all node provenance remain borrowed.
    pub fn compare_with<'a, 'b>(&'a self, new: &'b Self) -> ValueShapeComparison<'a, 'b> {
        ValueShapeComparison {
            old: self,
            new,
            frames: Vec::with_capacity(crate::bounds::MAX_DURABLE_VALUE_DEPTH),
            remaining: crate::bounds::MAX_IMAGE_BYTES,
        }
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

    /// The node for a dense struct value over already-minted leaves, each with its
    /// declared name, in declaration order.
    ///
    /// A leaf outside this arena's preserved provenance — an id from an independently
    /// minted arena or invalidated by truncation — is [`DraftStateError::ForeignDraft`],
    /// never an out-of-range index. An empty leaf name is [`DraftStateError::CarrierDomain`]:
    /// no declaration spells one, and a nameless leaf would identify nothing.
    pub fn struct_shape(
        &mut self,
        leaves: Vec<ValueShapeLeaf>,
    ) -> Result<ValueShapeNodeId, DraftStateError> {
        refuse_unnamed(&leaves)?;
        self.intern(ValueShapeNode::Struct(leaves))
    }

    /// The node for a closed enum value: its sum identity and, per variant in
    /// declaration order, its member identity and already-minted, named payload leaves.
    ///
    /// Checked exactly like [`Self::struct_shape`] over every payload leaf.
    pub fn enum_shape(
        &mut self,
        sum: LedgerIdBytes,
        members: Vec<ValueShapeEnumMember>,
    ) -> Result<ValueShapeNodeId, DraftStateError> {
        for member in &members {
            refuse_unnamed(&member.payload)?;
        }
        self.intern(ValueShapeNode::Enum { sum, members })
    }

    /// The longest path from `node` down to a scalar, counting `node` itself as one
    /// level, or `None` when `node` names nothing here. A top-level durable field value
    /// rooted here occupies exactly that many levels.
    pub fn depth(&self, node: ValueShapeNodeId) -> Option<usize> {
        self.store.depth_of(node).map(|depth| depth as usize)
    }

    /// Whether `node` authenticates an exact minted node of this arena.
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
        (0..count)
            .zip(self.store.stamps())
            .map(|(ordinal, stamp)| ValueShapeNodeId { ordinal, stamp })
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
        let store = &self.store;
        self.interned.retain(|_, head| {
            while head.index() >= len {
                let Some(predecessor) = store.fingerprint_predecessor(*head) else {
                    return false;
                };
                *head = predecessor;
            }
            true
        });
        self.store.truncate(len);
    }

    /// Mint `node`, or return the id of the structurally identical node already held.
    ///
    /// Every reference `node` carries must already be minted *here*: its depth is then
    /// final, so one `max` over the direct references is the exact longest path and the
    /// arena stays acyclic by construction. An arena is reachable from another crate
    /// through [`crate::product::DurableContractGraph::value_shapes_mut`], so both
    /// premises are checked rather than assumed — an id from an independently minted
    /// arena and an arena at its carrier ceiling are the closed builder-domain refusals,
    /// and neither mutates a single owner.
    fn intern(&mut self, node: ValueShapeNode) -> Result<ValueShapeNodeId, DraftStateError> {
        let fingerprint = ValueShapeFingerprint(self.interned.hasher().hash_one(&node));
        self.intern_with_fingerprint(node, fingerprint)
    }

    /// Intern using the already-computed randomized fingerprint. Hashing is split from
    /// insertion so the collision law can be forced by a unit test without a test-only
    /// production constructor.
    fn intern_with_fingerprint(
        &mut self,
        node: ValueShapeNode,
        fingerprint: ValueShapeFingerprint,
    ) -> Result<ValueShapeNodeId, DraftStateError> {
        let mut candidate = self.interned.get(&fingerprint).copied();
        while let Some(id) = candidate {
            if self.store.get(id) == Some(&node) {
                return Ok(id);
            }
            candidate = self.store.fingerprint_predecessor(id);
        }
        let depth = 1 + self.max_reference_depth(&node)?;
        // Refuse at `u32::MAX` rather than past it: the id minted here is the pre-push
        // length, so admitting that length would leave a post-push count no `u32` holds
        // and `nodes()` unable to name the last node.
        if self.store.len() >= u32::MAX as usize {
            return Err(DraftStateError::CarrierDomain);
        }
        let id = ValueShapeNodeId {
            ordinal: u32::try_from(self.store.len()).map_err(|_| DraftStateError::CarrierDomain)?,
            stamp: ValueShapeNodeStamp::mint()?,
        };
        let predecessor = self.interned.insert(fingerprint, id);
        self.store.push(
            node,
            depth,
            id.stamp,
            ValueShapeFingerprintPredecessor::from_id(predecessor),
        );
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

    fn max_depth(&self, leaves: &[ValueShapeLeaf]) -> Result<u32, DraftStateError> {
        let mut deepest = 0;
        for leaf in leaves {
            let depth = self
                .store
                .depth_of(leaf.shape)
                .ok_or(DraftStateError::ForeignDraft)?;
            deepest = deepest.max(depth);
        }
        Ok(deepest)
    }
}

/// Refuse a leaf with an empty name before anything is interned.
fn refuse_unnamed(leaves: &[ValueShapeLeaf]) -> Result<(), DraftStateError> {
    if leaves.iter().any(|leaf| leaf.name.is_empty()) {
        return Err(DraftStateError::CarrierDomain);
    }
    Ok(())
}

use node_store::ValueShapeNodeStore;

/// The arena's backing store, holding the minted nodes and their final depths behind
/// the only lookup that exists for them.
///
/// The vectors are private to this module, and a module's ancestors cannot reach a
/// descendant's private items, so no method of [`CanonicalValueShapeDag`] — public or
/// private — can index them on an unauthenticated ordinal. Every lookup compares the
/// caller's exact-node stamp with the stamp stored beside that ordinal, so an id minted
/// elsewhere or invalidated by truncation is `None` rather than a wrong-node binding or an
/// out-of-range abort reachable through the safe public surface.
mod node_store {
    use super::{
        ValueShapeFingerprintPredecessor, ValueShapeLeaf, ValueShapeNode, ValueShapeNodeId,
        ValueShapeNodeStamp,
    };

    #[derive(Debug, Clone, Default)]
    pub(super) struct ValueShapeNodeStore {
        nodes: Vec<ValueShapeNode>,
        /// The longest path from the node at the same ordinal down to a scalar, counting
        /// that node itself. Written once, when the node is minted, from references whose
        /// own depth is already final.
        depth: Vec<u32>,
        /// The exact-node stamp at the same ordinal. Truncation drops it and a later node
        /// at that ordinal receives a fresh stamp.
        stamps: Vec<ValueShapeNodeStamp>,
        /// The preceding node with the same randomized fingerprint, or the carrier's
        /// unmintable sentinel. This flat chain makes hash collisions allocation-free.
        fingerprint_predecessors: Vec<ValueShapeFingerprintPredecessor>,
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
            self.nodes.len() == other.nodes.len()
                && self
                    .nodes
                    .iter()
                    .zip(&other.nodes)
                    .all(|(left, right)| same_shape(left, right))
        }

        pub(super) fn get(&self, node: ValueShapeNodeId) -> Option<&ValueShapeNode> {
            self.authenticate(node)
                .and_then(|index| self.nodes.get(index))
        }

        pub(super) fn depth_of(&self, node: ValueShapeNodeId) -> Option<u32> {
            self.authenticate(node)
                .and_then(|index| self.depth.get(index).copied())
        }

        /// Append one minted node with its final depth, stamp, and collision predecessor.
        /// The four vectors move together, so an authenticated ordinal names one row.
        pub(super) fn push(
            &mut self,
            node: ValueShapeNode,
            depth: u32,
            stamp: ValueShapeNodeStamp,
            predecessor: ValueShapeFingerprintPredecessor,
        ) {
            self.nodes.push(node);
            self.depth.push(depth);
            self.stamps.push(stamp);
            self.fingerprint_predecessors.push(predecessor);
        }

        pub(super) fn truncate(&mut self, len: usize) {
            self.nodes.truncate(len);
            self.depth.truncate(len);
            self.stamps.truncate(len);
            self.fingerprint_predecessors.truncate(len);
        }

        pub(super) fn stamps(&self) -> impl Iterator<Item = ValueShapeNodeStamp> + '_ {
            self.stamps.iter().copied()
        }

        pub(super) fn fingerprint_predecessor(
            &self,
            node: ValueShapeNodeId,
        ) -> Option<ValueShapeNodeId> {
            let index = self.authenticate(node)?;
            let ordinal = self
                .fingerprint_predecessors
                .get(index)
                .copied()?
                .ordinal()?;
            let stamp = self.stamps.get(ordinal as usize).copied()?;
            Some(ValueShapeNodeId { ordinal, stamp })
        }

        fn authenticate(&self, node: ValueShapeNodeId) -> Option<usize> {
            let index = node.index();
            (self.stamps.get(index).copied() == Some(node.stamp)).then_some(index)
        }
    }

    /// Arena equality is semantic: independently built equal arenas carry different
    /// provenance stamps, so direct references compare by their topological ordinals.
    fn same_shape(left: &ValueShapeNode, right: &ValueShapeNode) -> bool {
        match (left, right) {
            (ValueShapeNode::Scalar(left), ValueShapeNode::Scalar(right)) => left == right,
            (ValueShapeNode::Struct(left), ValueShapeNode::Struct(right)) => {
                same_references(left, right)
            }
            (
                ValueShapeNode::Enum {
                    sum: left_sum,
                    members: left_members,
                },
                ValueShapeNode::Enum {
                    sum: right_sum,
                    members: right_members,
                },
            ) => {
                left_sum == right_sum
                    && left_members.len() == right_members.len()
                    && left_members.iter().zip(right_members).all(|(left, right)| {
                        left.id == right.id && same_references(&left.payload, &right.payload)
                    })
            }
            _ => false,
        }
    }

    fn same_references(left: &[ValueShapeLeaf], right: &[ValueShapeLeaf]) -> bool {
        left.len() == right.len()
            && left.iter().zip(right).all(|(left, right)| {
                left.name == right.name && left.shape.index() == right.shape.index()
            })
    }

    #[cfg(test)]
    mod tests {
        use super::ValueShapeNodeStore;

        /// Exhaustively name the private store fields. Adding a field requires this
        /// destructure to change, so the retained per-node representation is reviewed
        /// with it.
        #[test]
        fn the_priced_store_names_all_of_its_fields() {
            let _ = |value: &ValueShapeNodeStore| {
                let ValueShapeNodeStore {
                    nodes,
                    depth,
                    stamps,
                    fingerprint_predecessors,
                } = value;
                let _ = (nodes, depth, stamps, fingerprint_predecessors);
            };
        }
    }
}

impl ValueShapeNodeId {
    pub(crate) fn index(self) -> usize {
        self.ordinal as usize
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
pub(crate) trait ImageByteSink {
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
pub(crate) enum ValueShapeWireForm {
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
/// The marker before each struct leaf and payload leaf. It sits where the name-free
/// grammar placed a value tag, and it is outside the value-tag set, so no leaf position
/// parses under both grammars.
const VSHAPE_LEAF: u8 = 3;

/// One unit of pending expansion work.
///
/// An enum's variants are written one after another, each with its own header before its
/// payload, so a variant is its own work item rather than a position inside its node's
/// header. A variant task borrows the variant from the arena the expansion walks, so there
/// is no second lookup to get wrong and no "resume after children" continuation. Worklist
/// length depends on fan-out and scheduling as well as nesting.
enum ExpandTask<'a> {
    Node(ValueShapeNodeId),
    Leaf(&'a ValueShapeLeaf),
    EnumMember(&'a ValueShapeEnumMember),
}

/// Write the expanded bytes of the value shape rooted at `root` into `sink`, or refuse a
/// shape stating an arity no v0 wire form can spell.
///
/// The expansion is iterative and direct-to-sink: no expanded tree is built, and no
/// intermediate buffer holds one. Each work iteration checks whether the sink is full;
/// a scheduling arm can append children before the next check. Expansion otherwise
/// ends when the worklist is empty or a reference is refused.
///
/// `root` is a caller-supplied id, so the lookup authenticates its exact-node stamp: a
/// root from independent provenance or invalidated by truncation refuses rather than
/// aborts. Nested references carry the arena's own provenance, so that arm answers only
/// for a graph the declaration recheck would already have refused.
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
                    tasks.extend(leaves.iter().rev().map(ExpandTask::Leaf));
                }
                ValueShapeNode::Enum { sum, members } => {
                    sink.push(VSHAPE_ENUM);
                    push_identity(sink, form, IDREF_SUM, sum);
                    push_u16(sink, wire_count(members.len())?);
                    tasks.extend(members.iter().rev().map(ExpandTask::EnumMember));
                }
            },
            ExpandTask::Leaf(leaf) => {
                sink.push(VSHAPE_LEAF);
                push_u16(sink, wire_count(leaf.name.len())?);
                sink.extend_bytes(leaf.name.as_bytes());
                tasks.push(ExpandTask::Node(leaf.shape));
            }
            ExpandTask::EnumMember(member) => {
                push_identity(sink, form, IDREF_MEMBER, &member.id);
                push_u16(sink, wire_count(member.payload.len())?);
                tasks.extend(member.payload.iter().rev().map(ExpandTask::Leaf));
            }
        }
    }
    Ok(())
}

/// The wire's `u16` count for a value shape's `count` positions or a leaf name's byte
/// length, or [`DurableGraphTooLarge`] for a length no v0 wire form can spell.
///
/// Every arity a durable program states is bounded far below the wire's width —
/// [`crate::bounds::MAX_STRUCT_LEAVES`], [`crate::bounds::MAX_VARIANTS`], and
/// [`crate::bounds::MAX_PAYLOAD_FIELDS`] are all at most 256, a leaf name is at most
/// [`crate::bounds::MAX_STRING_BYTES`], and the whole arena is rechecked against them
/// before anything is encoded. An arena is public, though, so a
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
    use crate::fixtures::id;

    fn leaf(name: &str, shape: ValueShapeNodeId) -> ValueShapeLeaf {
        ValueShapeLeaf::new(name, shape)
    }

    /// Leaves named `f0`, `f1`, … by position, for tests whose subject is not the names.
    fn named(shapes: Vec<ValueShapeNodeId>) -> Vec<ValueShapeLeaf> {
        shapes
            .into_iter()
            .enumerate()
            .map(|(index, shape)| leaf(&format!("f{index}"), shape))
            .collect()
    }

    #[test]
    fn paired_values_authenticate_arenas_and_compare_late_enum_payloads() {
        let mut old = CanonicalValueShapeDag::new();
        let mut new = CanonicalValueShapeDag::new();
        let left = old.scalar(Scalar::Int).expect("scalar");
        let right = new.scalar(Scalar::Int).expect("independent scalar");
        let changed = new.scalar(Scalar::Bool).expect("changed scalar");
        let before = old
            .enum_shape(
                id(1),
                vec![
                    ValueShapeEnumMember::new(id(2), vec![]),
                    ValueShapeEnumMember::new(id(3), named(vec![left])),
                ],
            )
            .expect("enum");
        let same = new
            .enum_shape(
                id(1),
                vec![
                    ValueShapeEnumMember::new(id(2), vec![]),
                    ValueShapeEnumMember::new(id(3), named(vec![right])),
                ],
            )
            .expect("enum");
        let after = new
            .enum_shape(
                id(1),
                vec![
                    ValueShapeEnumMember::new(id(2), vec![]),
                    ValueShapeEnumMember::new(id(3), named(vec![changed])),
                ],
            )
            .expect("changed enum");
        let mut compare = old.compare_with(&new);
        assert_eq!(compare.same(before, same), Some(true));
        assert_eq!(compare.same(before, after), Some(false));
        assert_eq!(compare.same(right, left), None);
        assert_eq!(compare.same(left, right), Some(true));
        compare.remaining = 1;
        assert_eq!(compare.same(left, right), Some(true));
        assert_eq!(compare.same(left, right), None);
    }

    #[test]
    fn paired_value_scratch_is_depth_bounded_and_reused() {
        let mut graph = CanonicalValueShapeDag::new();
        let mut value = graph.scalar(Scalar::Int).expect("scalar");
        for _ in 1..crate::bounds::MAX_DURABLE_VALUE_DEPTH {
            value = graph
                .struct_shape(named(vec![value]))
                .expect("nested value");
        }
        let too_deep = graph
            .struct_shape(named(vec![value]))
            .expect("caller can state excess depth");
        let mut compare = graph.compare_with(&graph);
        let storage = compare.frames.as_ptr();
        assert_eq!(compare.same(value, value), Some(true));
        assert_eq!(compare.same(value, value), Some(true));
        assert_eq!(compare.frames.as_ptr(), storage);
        assert_eq!(
            compare.frames.capacity(),
            crate::bounds::MAX_DURABLE_VALUE_DEPTH
        );
        assert_eq!(compare.same(too_deep, too_deep), None);
    }

    #[test]
    fn stamp_domain_exhaustion_is_the_typed_carrier_refusal() {
        let next = AtomicU64::new(u64::MAX - 1);
        assert_eq!(
            ValueShapeNodeStamp::mint_from(&next),
            Ok(ValueShapeNodeStamp(u64::MAX - 1)),
        );
        assert_eq!(next.load(Ordering::Relaxed), u64::MAX);
        assert_eq!(
            ValueShapeNodeStamp::mint_from(&next),
            Err(DraftStateError::CarrierDomain),
        );
        assert_eq!(
            next.load(Ordering::Relaxed),
            u64::MAX,
            "a refusal leaves the exhausted counter unchanged",
        );
    }

    #[test]
    fn fingerprint_collisions_retain_distinct_nodes_and_rewind_on_truncate() {
        let collision = ValueShapeFingerprint(7);
        let mut dag = CanonicalValueShapeDag::new();
        let int = dag
            .intern_with_fingerprint(ValueShapeNode::Scalar(Scalar::Int), collision)
            .expect("the first colliding node mints");
        let stale_text = dag
            .intern_with_fingerprint(ValueShapeNode::Scalar(Scalar::Text), collision)
            .expect("the second colliding node mints");

        assert_eq!(
            dag.intern_with_fingerprint(ValueShapeNode::Scalar(Scalar::Int), collision),
            Ok(int),
        );
        assert_eq!(
            dag.intern_with_fingerprint(ValueShapeNode::Scalar(Scalar::Text), collision),
            Ok(stale_text),
        );
        assert_eq!(dag.len(), 2, "a collision does not merge unequal nodes");

        dag.truncate(1);
        assert_eq!(
            dag.intern_with_fingerprint(ValueShapeNode::Scalar(Scalar::Int), collision),
            Ok(int),
            "truncation rewinds the bucket head to its retained predecessor",
        );
        let replacement_text = dag
            .intern_with_fingerprint(ValueShapeNode::Scalar(Scalar::Text), collision)
            .expect("the truncated collision can be minted again");
        assert_ne!(
            replacement_text, stale_text,
            "the remint receives a fresh stamp"
        );
        assert_eq!(replacement_text.index(), stale_text.index());
    }

    /// Exhaustively name the arena, node, and leaf fields. Adding a field requires these
    /// destructures to change, so the retained per-node representation is reviewed with
    /// it. The private backing store has its own exhaustive destructure beside its owner.
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
        let _ = |value: &ValueShapeLeaf| {
            let ValueShapeLeaf { name, shape } = value;
            let _ = (name, shape);
        };
    }

    /// An id from one independently minted arena names no node in another, and an arena
    /// is reachable from another crate through `DurableContractGraph::value_shapes_mut`.
    /// Both composite minters therefore refuse it as [`DraftStateError::ForeignDraft`]
    /// and leave the target arena byte-for-byte unchanged — the id is never used as an
    /// unauthenticated index into the depth vector.
    #[test]
    fn a_foreign_leaf_is_the_typed_refusal_and_mutates_no_node() {
        let mut minting = CanonicalValueShapeDag::new();
        let foreign = minting.scalar(Scalar::Int).expect("the test arena mints");

        let mut empty = CanonicalValueShapeDag::new();
        assert_eq!(
            empty.struct_shape(named(vec![foreign])),
            Err(DraftStateError::ForeignDraft),
        );
        assert_eq!(
            empty.enum_shape(
                id(1),
                vec![ValueShapeEnumMember::new(id(2), named(vec![foreign]))]
            ),
            Err(DraftStateError::ForeignDraft),
        );
        assert_eq!(empty, CanonicalValueShapeDag::new(), "no node was minted");
        assert_eq!(empty.len(), 0);

        // The exact same-ordinal foreign case: `foreign` denotes Int in `minting`, while
        // ordinal zero denotes Text in `populated`. Range-checking alone would silently
        // bind the foreign leaf to Text and mint the wrong composite.
        let mut populated = CanonicalValueShapeDag::new();
        let local_text = populated
            .scalar(Scalar::Text)
            .expect("the test arena mints");
        assert_eq!(
            populated.struct_shape(named(vec![foreign])),
            Err(DraftStateError::ForeignDraft),
        );
        assert_eq!(populated.len(), 1, "the foreign leaf minted no composite");
        assert_eq!(
            populated.view(local_text),
            Some(ValueShapeView::Scalar(Scalar::Text)),
        );
    }

    /// Truncation may reuse an ordinal but never an exact-node stamp. A handle to the
    /// discarded node therefore stays stale after a different node takes its position.
    #[test]
    fn a_truncated_node_id_cannot_bind_to_the_replacement_at_its_ordinal() {
        let mut dag = CanonicalValueShapeDag::new();
        let stale_int = dag.scalar(Scalar::Int).expect("the test arena mints");
        dag.truncate(0);
        let replacement_text = dag.scalar(Scalar::Text).expect("the test arena re-mints");

        assert_eq!(
            stale_int.index(),
            replacement_text.index(),
            "the ordinal was reused"
        );
        assert_ne!(
            stale_int, replacement_text,
            "the exact-node stamp was not reused"
        );
        assert_eq!(dag.depth(stale_int), None);
        assert_eq!(dag.view(stale_int), None);
        assert!(!dag.contains(stale_int));
        assert_eq!(
            dag.struct_shape(named(vec![stale_int])),
            Err(DraftStateError::ForeignDraft),
        );
        assert_eq!(dag.len(), 1, "the stale leaf minted no composite");
        assert_eq!(
            dag.view(replacement_text),
            Some(ValueShapeView::Scalar(Scalar::Text)),
        );
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
            .struct_shape(named(vec![int, int]))
            .expect("the test arena mints");

        // The positive arm first: against its own arena every lookup answers, so no arm
        // below can pass by refusing everything.
        assert_eq!(minting.depth(foreign), Some(2));
        let Some(ValueShapeView::Struct(leaves)) = minting.view(foreign) else {
            panic!("the struct node reads back as a struct");
        };
        assert_eq!(
            leaves
                .iter()
                .map(|leaf| (leaf.name(), leaf.shape()))
                .collect::<Vec<_>>(),
            [("f0", int), ("f1", int)],
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

        // Populate the target through the same ordinal so these are authentication
        // checks, not only range checks.
        let mut target = CanonicalValueShapeDag::new();
        let text = target.scalar(Scalar::Text).expect("the target mints");
        let local = target
            .struct_shape(named(vec![text, text]))
            .expect("the target mints the same ordinal");
        assert_eq!(foreign.index(), local.index());
        assert_eq!(target.depth(foreign), None);
        assert_eq!(target.view(foreign), None);
        assert!(!target.contains(foreign));
        let mut refused: Vec<u8> = Vec::new();
        assert_eq!(
            expand(
                &target,
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
            .struct_shape(named(vec![int, int]))
            .expect("the test arena mints");
        let again = dag.scalar(Scalar::Int).expect("the test arena mints");
        let second = dag
            .struct_shape(named(vec![again, int]))
            .expect("the test arena mints");
        assert_eq!(first, second);
        assert_eq!(dag.len(), 2, "one scalar node and one struct node");

        let mut independently_built = CanonicalValueShapeDag::new();
        let other_int = independently_built
            .scalar(Scalar::Int)
            .expect("the second arena mints");
        independently_built
            .struct_shape(named(vec![other_int, other_int]))
            .expect("the second arena mints");
        assert_eq!(
            dag, independently_built,
            "arena equality ignores provenance stamps"
        );

        let mut renamed = CanonicalValueShapeDag::new();
        let renamed_int = renamed.scalar(Scalar::Int).expect("the third arena mints");
        renamed
            .struct_shape(vec![leaf("f0", renamed_int), leaf("g", renamed_int)])
            .expect("the third arena mints");
        assert_ne!(dag, renamed, "arena equality compares leaf names");
    }

    /// Depth is the longest path down to a scalar, counting the node itself.
    #[test]
    fn depth_is_the_longest_path_to_a_scalar() {
        let mut dag = CanonicalValueShapeDag::new();
        let int = dag.scalar(Scalar::Int).expect("the test arena mints");
        assert_eq!(dag.depth(int), Some(1));
        let pair = dag
            .struct_shape(named(vec![int, int]))
            .expect("the test arena mints");
        assert_eq!(dag.depth(pair), Some(2));
        // A struct holding both the scalar and the pair measures the longer branch.
        let mixed = dag
            .struct_shape(named(vec![int, pair]))
            .expect("the test arena mints");
        assert_eq!(dag.depth(mixed), Some(3));
    }

    /// The same node reached at two different depths keeps one depth — its own — so a
    /// shallow field value and a deep one are decided independently and correctly.
    #[test]
    fn a_shared_node_carries_one_depth_whatever_reaches_it() {
        let mut dag = CanonicalValueShapeDag::new();
        let int = dag.scalar(Scalar::Int).expect("the test arena mints");
        let shared = dag
            .struct_shape(named(vec![int]))
            .expect("the test arena mints");
        let mut deep = shared;
        for _ in 0..10 {
            deep = dag
                .struct_shape(named(vec![deep]))
                .expect("the test arena mints");
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
            .struct_shape(named(vec![int]))
            .expect("the test arena mints");
        let mut chain = shared;
        for _ in 0..5 {
            chain = deep_first
                .struct_shape(named(vec![chain]))
                .expect("the test arena mints");
        }
        let deep_first_pair = (deep_first.depth(shared), deep_first.depth(chain));

        let mut shallow_first = CanonicalValueShapeDag::new();
        let int = shallow_first
            .scalar(Scalar::Int)
            .expect("the test arena mints");
        let shared = shallow_first
            .struct_shape(named(vec![int]))
            .expect("the test arena mints");
        let _ = shallow_first.depth(shared);
        let mut chain = shared;
        for _ in 0..5 {
            chain = shallow_first
                .struct_shape(named(vec![chain]))
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
                .struct_shape(named(vec![level; 4]))
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
                .struct_shape(named(vec![level; 4]))
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
            .struct_shape(named(vec![int; u16::MAX as usize + 1]))
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
            .enum_shape(
                id(1),
                vec![ValueShapeEnumMember::new(id(2), named(vec![int]))],
            )
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
        assert_eq!(&section[1..17], id(1).bytes());
    }

    /// A stored leaf is identified by its declared name at its position, so the
    /// comparison refuses a swapped or renamed struct leaf or payload leaf even when every
    /// leaf has the same scalar type, and accepts an identically named shape.
    #[test]
    fn comparison_distinguishes_leaf_names_and_order() {
        let mut old = CanonicalValueShapeDag::new();
        let mut new = CanonicalValueShapeDag::new();
        let old_int = old.scalar(Scalar::Int).expect("scalar");
        let new_int = new.scalar(Scalar::Int).expect("scalar");
        let pos = old
            .struct_shape(vec![leaf("x", old_int), leaf("y", old_int)])
            .expect("struct");
        let rect = old
            .enum_shape(
                id(1),
                vec![ValueShapeEnumMember::new(
                    id(2),
                    vec![leaf("width", old_int), leaf("height", old_int)],
                )],
            )
            .expect("enum");
        let same_pos = new
            .struct_shape(vec![leaf("x", new_int), leaf("y", new_int)])
            .expect("struct");
        let swapped_pos = new
            .struct_shape(vec![leaf("y", new_int), leaf("x", new_int)])
            .expect("struct");
        let renamed_pos = new
            .struct_shape(vec![leaf("z", new_int), leaf("y", new_int)])
            .expect("struct");
        let same_rect = new
            .enum_shape(
                id(1),
                vec![ValueShapeEnumMember::new(
                    id(2),
                    vec![leaf("width", new_int), leaf("height", new_int)],
                )],
            )
            .expect("enum");
        let swapped_rect = new
            .enum_shape(
                id(1),
                vec![ValueShapeEnumMember::new(
                    id(2),
                    vec![leaf("height", new_int), leaf("width", new_int)],
                )],
            )
            .expect("enum");
        let mut compare = old.compare_with(&new);
        assert_eq!(compare.same(pos, same_pos), Some(true));
        assert_eq!(compare.same(pos, swapped_pos), Some(false));
        assert_eq!(compare.same(pos, renamed_pos), Some(false));
        assert_eq!(compare.same(rect, same_rect), Some(true));
        assert_eq!(compare.same(rect, swapped_rect), Some(false));
    }

    /// Each leaf pair costs its old name's bytes plus one, so the allowance keeps
    /// counting old wire bytes: one unit short of a long name's cost is no verdict.
    #[test]
    fn comparison_charges_name_bytes_to_the_allowance() {
        let name = "n".repeat(100);
        let mut dag = CanonicalValueShapeDag::new();
        let int = dag.scalar(Scalar::Int).expect("scalar");
        let shape = dag.struct_shape(vec![leaf(&name, int)]).expect("struct");
        // The struct node, its leaf (marker plus name), and the scalar node.
        let cost = 1 + (1 + name.len()) + 1;
        let mut compare = dag.compare_with(&dag);
        compare.remaining = cost;
        assert_eq!(compare.same(shape, shape), Some(true));
        compare.remaining = cost - 1;
        assert_eq!(compare.same(shape, shape), None);
    }

    /// Interning is by name as well as shape: two structs over the same leaf types with
    /// different names or order are distinct nodes.
    #[test]
    fn interning_keeps_differently_named_structs_distinct() {
        let mut dag = CanonicalValueShapeDag::new();
        let int = dag.scalar(Scalar::Int).expect("scalar");
        let xy = dag
            .struct_shape(vec![leaf("x", int), leaf("y", int)])
            .expect("struct");
        let yx = dag
            .struct_shape(vec![leaf("y", int), leaf("x", int)])
            .expect("struct");
        let again = dag
            .struct_shape(vec![leaf("x", int), leaf("y", int)])
            .expect("struct");
        assert_ne!(xy, yx);
        assert_eq!(xy, again);
        assert_eq!(dag.len(), 3);
    }

    /// A nameless leaf identifies nothing, so neither composite mint accepts one, and the
    /// refusal mints no node.
    #[test]
    fn an_empty_leaf_name_is_refused_at_the_mint() {
        let mut dag = CanonicalValueShapeDag::new();
        let int = dag.scalar(Scalar::Int).expect("scalar");
        assert_eq!(
            dag.struct_shape(vec![leaf("x", int), leaf("", int)]),
            Err(DraftStateError::CarrierDomain),
        );
        assert_eq!(
            dag.enum_shape(
                id(1),
                vec![ValueShapeEnumMember::new(id(2), vec![leaf("", int)])]
            ),
            Err(DraftStateError::CarrierDomain),
        );
        assert_eq!(dag.len(), 1, "no composite was minted");
    }

    /// A leaf name longer than the wire's `u16` length is refused, never narrowed.
    #[test]
    fn expansion_refuses_a_leaf_name_the_wire_length_cannot_spell() {
        let mut dag = CanonicalValueShapeDag::new();
        let int = dag.scalar(Scalar::Int).expect("scalar");
        let shape = dag
            .struct_shape(vec![leaf(&"n".repeat(u16::MAX as usize + 1), int)])
            .expect("the arena itself does not bound a name");
        let mut bytes = Vec::new();
        assert_eq!(
            expand(&dag, shape, ValueShapeWireForm::DurableSection, &mut bytes),
            Err(DurableGraphTooLarge),
        );
        assert_eq!(bytes, vec![VSHAPE_STRUCT, 0, 1, VSHAPE_LEAF]);
    }

    /// Each leaf is spelled as its marker, its name's length and bytes, then its value,
    /// identically in both wire forms.
    #[test]
    fn a_leaf_spells_its_marker_and_name_before_its_value() {
        let mut dag = CanonicalValueShapeDag::new();
        let int = dag.scalar(Scalar::Int).expect("scalar");
        let shape = dag
            .struct_shape(vec![leaf("x", int), leaf("yy", int)])
            .expect("struct");
        let mut bytes = Vec::new();
        expand(&dag, shape, ValueShapeWireForm::ContractPayload, &mut bytes).expect("expand");
        let int_tag = Scalar::Int.tag();
        assert_eq!(
            bytes,
            [
                VSHAPE_STRUCT,
                0,
                2,
                VSHAPE_LEAF,
                0,
                1,
                b'x',
                VSHAPE_SCALAR,
                int_tag,
                VSHAPE_LEAF,
                0,
                2,
                b'y',
                b'y',
                VSHAPE_SCALAR,
                int_tag,
            ],
        );
    }
}
