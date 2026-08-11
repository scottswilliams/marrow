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
//! [`ValueShapeSink`], which may stop accepting bytes at a ceiling. A shape whose
//! expansion is larger than any image may be is therefore decided in the bytes the
//! sink admits, not in the bytes the expansion would have produced.

use std::collections::HashMap;

use crate::durable_id::LedgerIdBytes;
use crate::ty::Scalar;

/// A reference to one node of a [`CanonicalValueShapeDag`].
///
/// The ordinal is private: an id is obtained only by minting a node into an arena, so
/// it always refers to a node that exists, in the arena that minted it. There is
/// deliberately no way to build one from a raw integer, and no way to build a value
/// shape without an arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValueShapeNodeId(u32);

/// One distinct durable value shape. Nested positions are references, so this type
/// cannot express a tree and cloning it copies one level.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ValueShapeNode {
    Scalar(Scalar),
    /// A dense struct's leaves, positionally. Names are not identity.
    Struct(Vec<ValueShapeNodeId>),
    Enum {
        sum: LedgerIdBytes,
        members: Vec<EnumMemberNode>,
    },
}

/// One variant of a closed enum value: its `Member` ledger identity and its dense
/// payload leaves in declaration order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct EnumMemberNode {
    id: LedgerIdBytes,
    payload: Vec<ValueShapeNodeId>,
}

/// The program's distinct durable value shapes, each held once.
#[derive(Debug, Clone, Default)]
pub struct CanonicalValueShapeDag {
    nodes: Vec<ValueShapeNode>,
    /// `depth[i]` is the longest path from node `i` down to a scalar, counting node
    /// `i` itself. Written once, when the node is minted, from references whose own
    /// depth is already final.
    depth: Vec<u32>,
    interned: HashMap<ValueShapeNode, ValueShapeNodeId>,
}

impl PartialEq for CanonicalValueShapeDag {
    /// Two arenas are equal when they hold the same nodes in the same order. The
    /// interning map is derived from the nodes, and the depths are derived from the
    /// nodes, so neither participates.
    fn eq(&self, other: &Self) -> bool {
        self.nodes == other.nodes
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
        self.nodes.len()
    }

    /// Whether this arena holds no shapes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The node for a scalar value (a nominal's caller erases it to its base scalar
    /// first).
    pub fn scalar(&mut self, scalar: Scalar) -> ValueShapeNodeId {
        self.intern(ValueShapeNode::Scalar(scalar))
    }

    /// The node for a dense struct value over already-minted leaves, in positional
    /// order.
    pub fn struct_shape(&mut self, leaves: Vec<ValueShapeNodeId>) -> ValueShapeNodeId {
        self.intern(ValueShapeNode::Struct(leaves))
    }

    /// The node for a closed enum value: its sum identity and, per variant in
    /// declaration order, its member identity and already-minted payload leaves.
    pub fn enum_shape(
        &mut self,
        sum: LedgerIdBytes,
        members: Vec<(LedgerIdBytes, Vec<ValueShapeNodeId>)>,
    ) -> ValueShapeNodeId {
        let members = members
            .into_iter()
            .map(|(id, payload)| EnumMemberNode { id, payload })
            .collect();
        self.intern(ValueShapeNode::Enum { sum, members })
    }

    /// The longest path from `node` down to a scalar, counting `node` itself as one
    /// level. A top-level durable field value rooted here occupies exactly this many
    /// levels.
    pub fn depth(&self, node: ValueShapeNodeId) -> usize {
        self.depth[node.index()] as usize
    }

    /// The direct fan-out of `node`: its struct leaf count, or its enum variant count,
    /// or zero for a scalar. The value bounds are rechecked per distinct node, so a
    /// shape shared by a thousand fields is measured once.
    pub fn fan_out(&self, node: ValueShapeNodeId) -> usize {
        match &self.nodes[node.index()] {
            ValueShapeNode::Scalar(_) => 0,
            ValueShapeNode::Struct(leaves) => leaves.len(),
            ValueShapeNode::Enum { members, .. } => members.len(),
        }
    }

    /// The payload widths of `node`'s enum variants in declaration order, or an empty
    /// slice iterator for a scalar or struct node.
    pub fn payload_widths(&self, node: ValueShapeNodeId) -> impl Iterator<Item = usize> + '_ {
        let members: &[EnumMemberNode] = match &self.nodes[node.index()] {
            ValueShapeNode::Enum { members, .. } => members,
            _ => &[],
        };
        members.iter().map(|member| member.payload.len())
    }

    /// Every node of this arena, in minting order — which is a topological order, so a
    /// node always follows the nodes it references. A whole-arena bound recheck walks
    /// this once and touches each distinct shape one time.
    pub fn nodes(&self) -> impl Iterator<Item = ValueShapeNodeId> {
        (0..self.nodes.len() as u32).map(ValueShapeNodeId)
    }

    /// Mint `node`, or return the id of the structurally identical node already held.
    ///
    /// Every reference `node` carries was minted earlier, so its depth is already
    /// final: one `max` over the direct references is the exact longest path, and the
    /// arena stays acyclic by construction.
    fn intern(&mut self, node: ValueShapeNode) -> ValueShapeNodeId {
        if let Some(existing) = self.interned.get(&node) {
            return *existing;
        }
        let depth = 1 + self.max_reference_depth(&node);
        let id = ValueShapeNodeId(self.nodes.len() as u32);
        self.interned.insert(node.clone(), id);
        self.nodes.push(node);
        self.depth.push(depth);
        id
    }

    /// The greatest depth among `node`'s direct references, or zero when it has none.
    fn max_reference_depth(&self, node: &ValueShapeNode) -> u32 {
        match node {
            ValueShapeNode::Scalar(_) => 0,
            ValueShapeNode::Struct(leaves) => self.max_depth(leaves),
            ValueShapeNode::Enum { members, .. } => members
                .iter()
                .map(|member| self.max_depth(&member.payload))
                .max()
                .unwrap_or(0),
        }
    }

    fn max_depth(&self, references: &[ValueShapeNodeId]) -> u32 {
        references
            .iter()
            .map(|reference| self.depth[reference.index()])
            .max()
            .unwrap_or(0)
    }
}

impl ValueShapeNodeId {
    fn index(self) -> usize {
        self.0 as usize
    }
}

/// A destination for the bytes of one expanded value shape.
///
/// The expansion of a shared shape can be exponentially larger than the shape itself,
/// so a sink may refuse further bytes once it has seen enough to decide its caller's
/// question. [`expand`] stops as soon as a sink reports [`ValueShapeSink::is_full`],
/// so the work an expansion costs is the bytes its sink accepts, never the bytes the
/// whole tree would occupy.
pub trait ValueShapeSink {
    fn push(&mut self, byte: u8);

    fn extend(&mut self, bytes: &[u8]);

    /// Whether this sink has all the bytes it needs. Once true it stays true.
    fn is_full(&self) -> bool {
        false
    }
}

impl ValueShapeSink for Vec<u8> {
    fn push(&mut self, byte: u8) {
        Vec::push(self, byte);
    }

    fn extend(&mut self, bytes: &[u8]) {
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

/// The `IDREF` kind tags for the two identities a value shape carries.
const IDREF_SUM: u8 = 5;
const IDREF_MEMBER: u8 = 6;

/// One unit of pending expansion work.
///
/// An enum's variants are written one after another, each with its own header before
/// its payload, so a variant is its own work item rather than a position inside its
/// node's header. Nothing follows a node's references, so the stack never holds a
/// "resume after children" continuation and its depth is bounded by the shape's own
/// nesting depth.
enum ExpandTask {
    Node(ValueShapeNodeId),
    EnumMember {
        node: ValueShapeNodeId,
        member: usize,
    },
}

/// Write the expanded bytes of the value shape rooted at `root` into `sink`.
///
/// The expansion is iterative and direct-to-sink: no expanded tree is built, and no
/// intermediate buffer holds one. It returns as soon as the whole shape is written or
/// the sink reports it is full, whichever comes first.
pub fn expand(
    dag: &CanonicalValueShapeDag,
    root: ValueShapeNodeId,
    form: ValueShapeWireForm,
    sink: &mut impl ValueShapeSink,
) {
    let mut tasks = vec![ExpandTask::Node(root)];
    while let Some(task) = tasks.pop() {
        if sink.is_full() {
            return;
        }
        match task {
            ExpandTask::Node(id) => match &dag.nodes[id.index()] {
                ValueShapeNode::Scalar(scalar) => {
                    sink.push(VSHAPE_SCALAR);
                    sink.push(scalar.tag());
                }
                ValueShapeNode::Struct(leaves) => {
                    sink.push(VSHAPE_STRUCT);
                    push_u16(sink, leaves.len());
                    tasks.extend(leaves.iter().rev().map(|leaf| ExpandTask::Node(*leaf)));
                }
                ValueShapeNode::Enum { sum, members } => {
                    sink.push(VSHAPE_ENUM);
                    push_identity(sink, form, IDREF_SUM, sum);
                    push_u16(sink, members.len());
                    tasks.extend(
                        (0..members.len())
                            .rev()
                            .map(|member| ExpandTask::EnumMember { node: id, member }),
                    );
                }
            },
            ExpandTask::EnumMember { node, member } => {
                let ValueShapeNode::Enum { members, .. } = &dag.nodes[node.index()] else {
                    // An `EnumMember` task is pushed only while reading an `Enum` node,
                    // and an arena is append-only, so this is unreachable.
                    return;
                };
                let member = &members[member];
                push_identity(sink, form, IDREF_MEMBER, &member.id);
                push_u16(sink, member.payload.len());
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
}

fn push_u16(sink: &mut impl ValueShapeSink, value: usize) {
    sink.extend(&(value as u16).to_be_bytes());
}

/// Write one ledger identity in the wire form's spelling: a kind-tagged length-prefixed
/// `IDREF` in the contract payload, raw 16 bytes in the DURABLE section.
fn push_identity(
    sink: &mut impl ValueShapeSink,
    form: ValueShapeWireForm,
    kind: u8,
    id: &LedgerIdBytes,
) {
    if form == ValueShapeWireForm::ContractPayload {
        sink.push(kind);
        sink.extend(&(16u64).to_be_bytes());
    }
    sink.extend(id.bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger_id(byte: u8) -> LedgerIdBytes {
        LedgerIdBytes::from_bytes([byte; 16])
    }

    /// A shape minted twice is one node: the arena is the canonical form, so equality
    /// of shapes is equality of ids.
    #[test]
    fn structurally_identical_shapes_share_one_node() {
        let mut dag = CanonicalValueShapeDag::new();
        let int = dag.scalar(Scalar::Int);
        let first = dag.struct_shape(vec![int, int]);
        let again = dag.scalar(Scalar::Int);
        let second = dag.struct_shape(vec![again, int]);
        assert_eq!(first, second);
        assert_eq!(dag.len(), 2, "one scalar node and one struct node");
    }

    /// Depth is the longest path down to a scalar, counting the node itself.
    #[test]
    fn depth_is_the_longest_path_to_a_scalar() {
        let mut dag = CanonicalValueShapeDag::new();
        let int = dag.scalar(Scalar::Int);
        assert_eq!(dag.depth(int), 1);
        let pair = dag.struct_shape(vec![int, int]);
        assert_eq!(dag.depth(pair), 2);
        // A struct holding both the scalar and the pair measures the longer branch.
        let mixed = dag.struct_shape(vec![int, pair]);
        assert_eq!(dag.depth(mixed), 3);
    }

    /// The same node reached at two different depths keeps one depth — its own — so a
    /// shallow field value and a deep one are decided independently and correctly.
    #[test]
    fn a_shared_node_carries_one_depth_whatever_reaches_it() {
        let mut dag = CanonicalValueShapeDag::new();
        let int = dag.scalar(Scalar::Int);
        let shared = dag.struct_shape(vec![int]);
        let mut deep = shared;
        for _ in 0..10 {
            deep = dag.struct_shape(vec![deep]);
        }
        assert_eq!(
            dag.depth(shared),
            2,
            "the shallow field value stays shallow"
        );
        assert_eq!(dag.depth(deep), 12);
    }

    /// The interning order is a topological order whichever order the caller mints in,
    /// so building the deep occurrence first and the shallow one second measures both
    /// identically.
    #[test]
    fn depth_does_not_depend_on_minting_order() {
        let mut deep_first = CanonicalValueShapeDag::new();
        let int = deep_first.scalar(Scalar::Int);
        let shared = deep_first.struct_shape(vec![int]);
        let mut chain = shared;
        for _ in 0..5 {
            chain = deep_first.struct_shape(vec![chain]);
        }
        let deep_first_pair = (deep_first.depth(shared), deep_first.depth(chain));

        let mut shallow_first = CanonicalValueShapeDag::new();
        let int = shallow_first.scalar(Scalar::Int);
        let shared = shallow_first.struct_shape(vec![int]);
        let _ = shallow_first.depth(shared);
        let mut chain = shared;
        for _ in 0..5 {
            chain = shallow_first.struct_shape(vec![chain]);
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
        let mut level = dag.scalar(Scalar::Int);
        for _ in 0..14 {
            level = dag.struct_shape(vec![level; 4]);
        }
        assert_eq!(dag.len(), 15, "one scalar plus fourteen struct levels");
        assert_eq!(dag.depth(level), 15);
    }

    /// A counting sink that stops at a ceiling.
    struct Ceiling {
        written: usize,
        ceiling: usize,
    }

    impl ValueShapeSink for Ceiling {
        fn push(&mut self, _byte: u8) {
            self.written += 1;
        }

        fn extend(&mut self, bytes: &[u8]) {
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
        let mut level = dag.scalar(Scalar::Int);
        for _ in 0..14 {
            level = dag.struct_shape(vec![level; 4]);
        }
        let mut sink = Ceiling {
            written: 0,
            ceiling: 4096,
        };
        expand(&dag, level, ValueShapeWireForm::DurableSection, &mut sink);
        assert!(
            sink.written <= 4096 + 8,
            "expansion overran the ceiling by more than one node's header: {}",
            sink.written,
        );
    }

    /// The two wire forms differ only in how a ledger identity is spelled: the contract
    /// payload tags and length-prefixes it, the DURABLE section writes it raw.
    #[test]
    fn the_two_wire_forms_differ_only_in_identity_spelling() {
        let mut dag = CanonicalValueShapeDag::new();
        let int = dag.scalar(Scalar::Int);
        let shape = dag.enum_shape(ledger_id(1), vec![(ledger_id(2), vec![int])]);

        let mut payload = Vec::new();
        expand(
            &dag,
            shape,
            ValueShapeWireForm::ContractPayload,
            &mut payload,
        );
        let mut section = Vec::new();
        expand(
            &dag,
            shape,
            ValueShapeWireForm::DurableSection,
            &mut section,
        );

        assert_eq!(payload.len(), section.len() + 2 * 9);
        assert_eq!(payload[0], VSHAPE_ENUM);
        assert_eq!(payload[1], IDREF_SUM);
        assert_eq!(section[0], VSHAPE_ENUM);
        assert_eq!(&section[1..17], ledger_id(1).bytes());
    }
}
