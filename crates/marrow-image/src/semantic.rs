//! The derived stable semantic path of a durable graph node.
//!
//! Every node of a program's durable graph — a root placement, a static `group`
//! namespace, a keyed `branch` placement, and each stored field — has a derived
//! stable [`SemanticPath`]: the ordered chain of kind-tagged ledger ids from the
//! application down to the node. A path is **index-free** — it is the chain of the
//! graph's entropy-minted [`LedgerIdBytes`], never a container table index — and it
//! **follows the ledger ids**, so a rename that moves a ledger anchor (its id
//! unchanged) leaves every node's path unchanged, while re-minting an id changes
//! exactly the paths that pass through it. Two nodes are the same place iff their
//! chains are equal.
//!
//! The path is the stable identity. It is derived from the same durable member
//! tree that backs the [`crate::DurableContractDescriptor`] over ledger ids, so the
//! compiler and the verifier reconstruct identical paths from the same graph. There
//! is deliberately no separate hashed `PathId`: the ledger-id chain is itself the
//! stable identity, and durable authority, evolution, tooling, and physical
//! encoding project from this one owner rather than minting a parallel path model.
//!
//! A [`SemanticPath`] names *which* graph node; what an operation does *at* that
//! node — observe the whole payload, read or write one field — is the separate
//! operation-target concern. Key columns are identity attributes of a placement,
//! not separately addressable nodes, so they are not path steps.

use crate::durable_id::LedgerIdBytes;

/// The kind of one step in a [`SemanticPath`], mirroring the ledger's frozen kind
/// space: the application root, a keyed placement (a store root or a nested
/// `branch` — both keyed nodes), a static `group` namespace, or a stored field.
///
/// The [`Self::ledger_kind`] byte is the same domain-separation tag the durable
/// contract identity uses for the corresponding `IDREF`, so a path step and the
/// graph's identity payload agree on how a node's kind is spelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SemanticStepKind {
    /// The program's single durable application root (`IDREF` kind 0).
    Application,
    /// A keyed placement: a store root or a nested `branch` (`IDREF` kind 3).
    Placement,
    /// A static `group` field-path namespace (`IDREF` kind 7).
    Group,
    /// A stored field (`IDREF` kind 2).
    Field,
    /// A managed index of a keyed root (`IDREF` kind 8).
    Index,
}

impl SemanticStepKind {
    /// The frozen ledger `IDREF` kind byte for this step, so a path spells a node's
    /// kind exactly as the durable-contract identity payload does.
    pub fn ledger_kind(self) -> u8 {
        match self {
            SemanticStepKind::Application => 0,
            SemanticStepKind::Placement => 3,
            SemanticStepKind::Group => 7,
            SemanticStepKind::Field => 2,
            SemanticStepKind::Index => 8,
        }
    }

    /// The step kind for a frozen ledger `IDREF` kind byte, or `None` for a byte
    /// outside the kind space a path step may carry. The inverse of
    /// [`Self::ledger_kind`]; the verifier decodes an untrusted site path's step
    /// kinds through this and rejects an unknown byte.
    pub fn from_ledger_kind(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(SemanticStepKind::Application),
            3 => Some(SemanticStepKind::Placement),
            7 => Some(SemanticStepKind::Group),
            2 => Some(SemanticStepKind::Field),
            8 => Some(SemanticStepKind::Index),
            _ => None,
        }
    }
}

/// What an operation site does *at* the graph node its [`SemanticPath`] names: the
/// closed operation-target set. `WholePayload` observes or writes a keyed placement's
/// whole entry; `FieldLeaf` reads or writes one stored field leaf; `GroupEntry`
/// observes or writes the whole materialized value of one unkeyed `group` node;
/// `IndexScan` is the nonunique progressive typed-prefix read of a managed index;
/// `IndexLookup` is the unique complete-key exact read of a managed index. The node the
/// path resolves to fixes which is legal — a placement admits `WholePayload`, a field
/// admits `FieldLeaf`, a group admits `GroupEntry`, a nonunique index admits
/// `IndexScan`, and a unique index admits `IndexLookup` — so the two together name a
/// site. There is no index *write* target: managed-index maintenance is compiler-owned
/// with no application opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticTarget {
    WholePayload,
    FieldLeaf,
    /// The whole materialized record value of one unkeyed `group` node: read as a unit,
    /// or replaced/erased under the group-scoped payload-only law. The group is
    /// addressed by its containing entry's key-path, like a whole payload, but scopes to
    /// the group's own field set.
    GroupEntry,
    /// The nonunique progressive typed-prefix read of a managed index: an incomplete
    /// prefix yields the next distinct component; the complete projection yields the
    /// source-root key. Runtime traversal lands at E05.
    IndexScan,
    /// The unique complete-key exact read of a managed index: it yields exactly the
    /// one matching source-root key or absent, never a sibling or a traversal.
    /// Runtime lookup lands at E05.
    IndexLookup,
}

/// One step of a [`SemanticPath`]: a node's kind-tagged entropy-minted ledger id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemanticStep {
    pub kind: SemanticStepKind,
    pub id: LedgerIdBytes,
}

impl SemanticStep {
    pub fn new(kind: SemanticStepKind, id: LedgerIdBytes) -> Self {
        Self { kind, id }
    }
}

/// The derived stable path identity of one durable graph node: the ordered chain of
/// kind-tagged ledger ids from the application to the node. A root's path is
/// `[application, root placement]`; a field's path extends its container's path with
/// the field step. The chain is the identity — it carries no source spelling and no
/// container index — so equality and ordering are structural over the ledger ids.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemanticPath {
    steps: Vec<SemanticStep>,
}

impl SemanticPath {
    /// Build a path from an application step and the chain of node steps below it.
    /// The application step is the shared root of every path in a graph.
    pub(crate) fn new(steps: Vec<SemanticStep>) -> Self {
        Self { steps }
    }

    /// Build a path from an explicit step chain. This is the producer-side
    /// constructor a compiler uses to spell the path of an operation site it emits
    /// into the image; the verifier does not trust such a path but resolves it
    /// against its own independently derived [`crate::SemanticNode`] set. The chain
    /// runs from the application step to the addressed node and must be non-empty.
    pub fn from_steps(steps: Vec<SemanticStep>) -> Self {
        debug_assert!(!steps.is_empty(), "a semantic path has at least one step");
        Self { steps }
    }

    /// This path's ordered steps, from the application root to the addressed node.
    pub fn steps(&self) -> &[SemanticStep] {
        &self.steps
    }

    /// The addressed node's own ledger id — the last step's id. Every path has at
    /// least the application step, so this is always defined.
    pub fn node_id(&self) -> LedgerIdBytes {
        self.steps.last().expect("a path has at least one step").id
    }

    /// A child path: this path extended by one more step. Used by the graph walker to
    /// descend into a group's or a branch's members.
    pub(crate) fn child(&self, step: SemanticStep) -> Self {
        let mut steps = self.steps.clone();
        steps.push(step);
        Self { steps }
    }
}

/// The kind of durable graph node a [`SemanticNode`] identifies. Distinct from
/// [`SemanticStepKind`]: a branch is its own node kind here, though its path step is
/// a [`SemanticStepKind::Placement`] like a root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SemanticNodeKind {
    Root,
    Group,
    Branch,
    Field,
    /// A managed index of a keyed root: a graph node with its own semantic path
    /// (the root path extended by the index step), read-only from source.
    Index,
}

/// A durable graph node paired with its derived [`SemanticPath`]. The compiler and
/// the verifier each enumerate these from the durable member tree; because the path
/// is the ledger-id chain, their enumerations agree node-for-node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticNode {
    pub kind: SemanticNodeKind,
    pub path: SemanticPath,
}
