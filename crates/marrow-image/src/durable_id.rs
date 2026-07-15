//! The `DurableContractId` durable-graph identity (kernel identity rule).
//!
//! A [`DurableContractId`] is the stable 32-byte identity of a program's whole
//! durable graph — the application, the roots, their key columns, and each root
//! record's stored field profile — computed over the graph's **ledger ids**, the
//! entropy-minted identities the committed `marrow.ids` artifact binds to each
//! durable declaration. Because the payload carries ids rather than names, a
//! rename (which moves a ledger anchor while its id stays) preserves the
//! contract identity, while every semantic graph change — a retyped key, a field
//! made required, a field added, removed, or re-minted — changes it. It crosses
//! the compiler → image → verifier boundary and will later cross the
//! store-admission boundary, so it is a distinct typed 32-byte domain-separated
//! SHA-256 over a length-delimited canonical payload, exactly as the kernel
//! identity rule requires: one owning phase (D00), one frozen `kind`, one
//! canonical payload, one known-answer test, and one independent-decoder
//! reconstruction test.
//!
//! The ledger ids themselves are the separate entropy-minted identity family;
//! this id is a deterministic hash *over* them. The compiler mints it and
//! carries it in the image; the verifier never trusts the carried bytes as
//! authoritative — it independently rebuilds the descriptor from the decoded
//! tables, recomputes the id, and rejects a mismatch. Anyone can mint a valid
//! id, so trust comes only from that recomputation.
//!
//! ```text
//! DurableContractId = SHA-256( KIND ‖ u64_be(len(payload)) ‖ payload )
//!   KIND    = b"marrow.durable.v0"
//!   payload = LP(lineage) ‖ LP(graph)
//!   LP(b)   = u64_be(b.len()) ‖ b
//!   lineage = the durable graph's package lineage. The local project root is the
//!             single tag byte 0x00; a dependency package is 0x01 ‖ <32-byte package
//!             id> at a later phase. The tag byte keeps the two disjoint, so packages
//!             are later breadth rather than an identity/format break — mirroring the
//!             `ExportId` lineage seam.
//!   graph   = u16_be(root_count)
//!             ‖ [ IDREF(0x00, application) when root_count > 0 ]
//!             ‖ root*                                          (roots in image order)
//!   root    = IDREF(0x03, placement) ‖ IDREF(0x01, product)
//!             ‖ u16_be(key_count) ‖ key*                       (key columns in tuple order)
//!             ‖ members                                        (the resource's durable member tree)
//!   members = u16_be(member_count) ‖ member*                  (in source declaration order)
//!   member  = u8(member_tag) ‖ member_body
//!     field(0)  = IDREF(0x02, field) ‖ u8(required?1:0) ‖ value
//!     group(1)  = IDREF(0x07, group) ‖ members
//!     branch(2) = IDREF(0x03, placement) ‖ u16_be(key_count) ‖ key* ‖ members
//!   key     = u8(key_scalar_tag) ‖ IDREF(0x04, key)
//!   value   = u8(value_tag) ‖ value_body                       (a durable field's stored value shape)
//!     scalar(0) = u8(scalar_tag)                               (a nominal erases to its base scalar)
//!     struct(1) = u16_be(leaf_count) ‖ value*                  (dense struct leaves, all required; names are not identity)
//!     enum(2)   = IDREF(0x05, sum) ‖ u16_be(member_count) ‖ evalue*
//!   evalue  = IDREF(0x06, member) ‖ u16_be(payload_count) ‖ value*   (one enum member: its id and dense payload leaves)
//!   IDREF(k, id) = u8(k) ‖ u64_be(16) ‖ id                     (kind-tagged, LP 16 bytes)
//! ```
//!
//! A durable field's stored `value` is drawn from the closed acyclic durable value
//! set: a nominal scalar (erased to its base scalar), a dense `struct` (its leaves
//! recorded positionally as shape bytes — a nested product leaf mints no ledger id
//! of its own, because the containing field is the renamable durable declaration),
//! a closed `enum` (`Option`/`Result`/a user `enum`, each carrying a sum identity
//! (kind 5) and one member identity (kind 6) per variant so append-only member
//! evolution has stable per-member codes), or an `Option`, which is itself a closed
//! enum (`none`/`some`). Collections and nested sparse/place/function/handle leaves
//! are not durable value leaves. Only a durable-reachable enum contributes sum and
//! member ids; a storeless enum stays ledger-free.
//!
//! A key tuple is length-prefixed, so a singleton root (`key_count = 0`) and a
//! composite root (`key_count > 1`) are the same shape as the ordinary
//! single-column root, and key-column order is part of the identity.
//!
//! A resource's durable shape is a **member tree**: its top-level fields plus any
//! static `group` field-path namespaces and keyed `branch` placements, each of
//! which recursively holds its own members. A group is an unkeyed namespace (a
//! `Group` identity); a branch is a keyed placement (its own `Root`-kind placement
//! identity and key tuple), so a nested keyed subtree is a distinct graph node
//! with a complete identity, just like a root. Member order is source declaration
//! order and is part of the identity. Only the flat single-column-keyed root with
//! no groups or branches is executable in this preview; the wider shapes complete
//! their identity and verify but run at E01.
//!
//! The `IDREF` kind tags mirror the ledger's frozen kind space (application 0,
//! product 1, field 2, root/branch placement 3, key 4, group 7; 5-6 reserved). An
//! empty graph (no roots) has no application component: a storeless project needs
//! no ledger, so its contract commits to nothing. Scalar tags are the frozen
//! [`Scalar::tag`] bytes. The `member_tag` bytes (field 0, group 1, branch 2) are
//! internal to this payload and independent of the ledger kind space. Operation
//! *sites* are deliberately excluded: they are derivable access points over the
//! graph, not part of its durable identity.

use sha2::{Digest, Sha256};

use crate::semantic::{
    SemanticNode, SemanticNodeKind, SemanticPath, SemanticStep, SemanticStepKind,
};
use crate::ty::Scalar;

/// The domain-separation tag for the durable-contract identity. Distinct from every
/// other Marrow identity's `kind`, so a `DurableContractId` can never collide with
/// an `ImageId` or `ExportId` computed over the same bytes.
pub const DURABLE_CONTRACT_KIND: &[u8; 17] = b"marrow.durable.v0";

/// The lineage of a durable graph declared in the local project root: the single tag
/// byte `0x00`. A dependency package's lineage begins with `0x01` at a later phase,
/// so the tag byte alone keeps local and package lineages disjoint.
const LOCAL_ROOT_LINEAGE: &[u8] = &[0x00];

/// The frozen `IDREF` kind tags, mirroring the ledger's kind space.
const IDREF_APPLICATION: u8 = 0;
const IDREF_PRODUCT: u8 = 1;
const IDREF_FIELD: u8 = 2;
const IDREF_ROOT: u8 = 3;
const IDREF_KEY: u8 = 4;
const IDREF_SUM: u8 = 5;
const IDREF_MEMBER: u8 = 6;
const IDREF_GROUP: u8 = 7;

/// The frozen value-shape tag bytes distinguishing a durable field's stored value
/// kinds within the canonical payload. Internal to this encoding and separate from
/// the ledger `IDREF` kind space.
const VSHAPE_SCALAR: u8 = 0;
const VSHAPE_STRUCT: u8 = 1;
const VSHAPE_ENUM: u8 = 2;

/// The frozen member-tag bytes distinguishing the three durable member kinds
/// within the canonical payload. They are internal to this encoding and separate
/// from the ledger `IDREF` kind space.
const MEMBER_FIELD: u8 = 0;
const MEMBER_GROUP: u8 = 1;
const MEMBER_BRANCH: u8 = 2;

/// An entropy-minted 128-bit ledger id as the image carries it: 16 opaque bytes.
/// The artifact-side semantics (anchors, tombstones, hex spelling) live with the
/// ledger owner; the image only transports and hashes the bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LedgerIdBytes([u8; 16]);

impl LedgerIdBytes {
    /// Wrap 16 raw id bytes.
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The 16 id bytes.
    pub fn bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

/// One stored field of a durable resource, group, or branch, as it contributes to
/// the contract identity: its ledger id, whether it is required, and its stored
/// value shape. The field's *name* is not part of the identity — a rename preserves
/// it — but its value shape is: retyping or restructuring the value changes the
/// contract id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableFieldShape {
    pub id: LedgerIdBytes,
    pub required: bool,
    pub value: DurableValueShape,
}

/// The stored value shape of a durable field, from the closed acyclic durable value
/// set. A nominal scalar erases to its base [`Scalar`]; a dense `struct` records its
/// leaves positionally (no per-leaf ledger id); a closed `enum` (including `Option`)
/// carries its sum identity and one member identity per variant. The tree is
/// recursive: a struct leaf or an enum member payload is itself a value shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurableValueShape {
    Scalar(Scalar),
    Struct(Vec<DurableValueShape>),
    Enum {
        sum: LedgerIdBytes,
        members: Vec<DurableEnumMemberShape>,
    },
}

/// One member (variant) of a durable-reachable enum, as it contributes to the
/// contract identity: its ledger id (kind 6) and its dense payload leaves in
/// declaration order. Member order is part of the identity, so append-only member
/// evolution preserves every existing member's id and position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableEnumMemberShape {
    pub id: LedgerIdBytes,
    pub payload: Vec<DurableValueShape>,
}

impl DurableValueShape {
    /// Append this value shape's canonical bytes (the `value` production above).
    /// Shared by the descriptor's identity payload and the image DURABLE section, so
    /// both spell a durable field's value one way.
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            DurableValueShape::Scalar(scalar) => {
                out.push(VSHAPE_SCALAR);
                out.push(scalar.tag());
            }
            DurableValueShape::Struct(leaves) => {
                out.push(VSHAPE_STRUCT);
                out.extend_from_slice(&(leaves.len() as u16).to_be_bytes());
                for leaf in leaves {
                    leaf.encode(out);
                }
            }
            DurableValueShape::Enum { sum, members } => {
                out.push(VSHAPE_ENUM);
                push_idref(out, IDREF_SUM, sum);
                out.extend_from_slice(&(members.len() as u16).to_be_bytes());
                for member in members {
                    push_idref(out, IDREF_MEMBER, &member.id);
                    out.extend_from_slice(&(member.payload.len() as u16).to_be_bytes());
                    for leaf in &member.payload {
                        leaf.encode(out);
                    }
                }
            }
        }
    }
}

/// One key column of a durable root or branch placement, as it contributes to the
/// contract identity: its orderable durable-key scalar and its ledger id. Column
/// order is the declared tuple order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableKeyShape {
    pub scalar: Scalar,
    pub id: LedgerIdBytes,
}

/// One static field-path namespace (`group`) as it contributes to the contract
/// identity: its `Group` ledger id and its own ordered member tree. A group is an
/// unkeyed pathing construct; it stores no data of its own beyond its members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableGroupShape {
    pub id: LedgerIdBytes,
    pub members: Vec<DurableMemberShape>,
}

/// One keyed subtree (`branch`) as it contributes to the contract identity: its
/// own placement id, its ordered key tuple, and its own member tree. A branch is a
/// distinct keyed graph node nested under its containing resource, branch, or
/// group — the same placement/key shape as a root, without a separate product.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableBranchShape {
    pub placement: LedgerIdBytes,
    pub keys: Vec<DurableKeyShape>,
    pub members: Vec<DurableMemberShape>,
}

/// One member of a durable resource's shape, in source declaration order: a stored
/// scalar field, a static `group` namespace, or a keyed `branch` placement. The
/// tree is recursive — groups and branches carry their own members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurableMemberShape {
    Field(DurableFieldShape),
    Group(DurableGroupShape),
    Branch(DurableBranchShape),
}

/// One durable root, as it contributes to the contract identity: its placement id,
/// the stored product's id, its ordered key tuple (empty for a singleton root),
/// and the ordered member tree of its resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableRootShape {
    pub placement: LedgerIdBytes,
    pub product: LedgerIdBytes,
    pub keys: Vec<DurableKeyShape>,
    pub members: Vec<DurableMemberShape>,
}

/// The canonical descriptor of a program's durable graph. This is the single owner
/// of the contract's canonical payload: the compiler builds one from its resolved
/// roots, record types, and ledger ids, and the verifier independently builds one
/// from the decoded image tables. Both call
/// [`DurableContractDescriptor::contract_id`], so there is exactly one canonical
/// encoding, and agreement between the two is a recomputation rather than a trusted
/// transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableContractDescriptor {
    application: Option<LedgerIdBytes>,
    roots: Vec<DurableRootShape>,
}

impl DurableContractDescriptor {
    /// Build a descriptor for a durable graph: the application's ledger id and the
    /// roots in image order. A graph with roots always has an application identity.
    pub fn new(application: LedgerIdBytes, roots: Vec<DurableRootShape>) -> Self {
        Self {
            application: Some(application),
            roots,
        }
    }

    /// The canonical descriptor of the empty durable graph: a storeless project has
    /// no ledger, no application id, and a well-defined stable contract id.
    pub fn empty() -> Self {
        Self {
            application: None,
            roots: Vec::new(),
        }
    }

    /// The stable 32-byte identity of this durable graph in the local project root.
    pub fn contract_id(&self) -> DurableContractId {
        DurableContractId::compute(LOCAL_ROOT_LINEAGE, &self.encode_graph())
    }

    /// Enumerate every durable graph node paired with its derived [`SemanticPath`]:
    /// each root placement, static `group` namespace, keyed `branch` placement, and
    /// stored field, in a stable pre-order (a node before its descendants, members in
    /// declaration order). The path is the chain of kind-tagged ledger ids from the
    /// application to the node, so a rename that only moves ledger anchors leaves
    /// every path unchanged while any structural or id change alters exactly the
    /// paths through it. The empty graph yields no nodes.
    ///
    /// This is the single owner of the derived path identity; the compiler builds a
    /// descriptor from its resolved graph and the verifier rebuilds one from the
    /// decoded image tables, so both enumerate identical paths.
    pub fn semantic_nodes(&self) -> Vec<SemanticNode> {
        let Some(application) = self.application else {
            return Vec::new();
        };
        let mut nodes = Vec::new();
        let app_step = SemanticStep::new(SemanticStepKind::Application, application);
        for root in &self.roots {
            let root_path = SemanticPath::new(vec![
                app_step,
                SemanticStep::new(SemanticStepKind::Placement, root.placement),
            ]);
            nodes.push(SemanticNode {
                kind: SemanticNodeKind::Root,
                path: root_path.clone(),
            });
            collect_member_nodes(&root_path, &root.members, &mut nodes);
        }
        nodes
    }

    /// The canonical graph bytes (the `graph` production above). Length-delimited so
    /// the whole is fed as one `LP(graph)` component of the payload.
    fn encode_graph(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.roots.len() as u16).to_be_bytes());
        if let Some(application) = &self.application {
            push_idref(&mut out, IDREF_APPLICATION, application);
        }
        for root in &self.roots {
            push_idref(&mut out, IDREF_ROOT, &root.placement);
            push_idref(&mut out, IDREF_PRODUCT, &root.product);
            push_keys(&mut out, &root.keys);
            push_members(&mut out, &root.members);
        }
        out
    }
}

/// Walk one member tree under `container`'s path, appending a [`SemanticNode`] for
/// each field, group, and branch in declaration order (a node before its
/// descendants). A group and a branch each extend the path with their own step and
/// recurse; a field is a leaf. Key columns are placement identity attributes, not
/// nodes, so they are not walked.
fn collect_member_nodes(
    container: &SemanticPath,
    members: &[DurableMemberShape],
    nodes: &mut Vec<SemanticNode>,
) {
    for member in members {
        match member {
            DurableMemberShape::Field(field) => {
                nodes.push(SemanticNode {
                    kind: SemanticNodeKind::Field,
                    path: container.child(SemanticStep::new(SemanticStepKind::Field, field.id)),
                });
            }
            DurableMemberShape::Group(group) => {
                let path = container.child(SemanticStep::new(SemanticStepKind::Group, group.id));
                nodes.push(SemanticNode {
                    kind: SemanticNodeKind::Group,
                    path: path.clone(),
                });
                collect_member_nodes(&path, &group.members, nodes);
            }
            DurableMemberShape::Branch(branch) => {
                let path = container.child(SemanticStep::new(
                    SemanticStepKind::Placement,
                    branch.placement,
                ));
                nodes.push(SemanticNode {
                    kind: SemanticNodeKind::Branch,
                    path: path.clone(),
                });
                collect_member_nodes(&path, &branch.members, nodes);
            }
        }
    }
}

/// Append `u16_be(count) ‖ [u8(scalar_tag) ‖ IDREF(key)]*` — a placement's key
/// tuple, shared by roots and branches. Column order is load-bearing.
fn push_keys(out: &mut Vec<u8>, keys: &[DurableKeyShape]) {
    out.extend_from_slice(&(keys.len() as u16).to_be_bytes());
    for key in keys {
        out.push(key.scalar.tag());
        push_idref(out, IDREF_KEY, &key.id);
    }
}

/// Append a member tree: `u16_be(count) ‖ member*`, each member a tag byte and its
/// body. Recurses through groups and branches so a whole durable shape has one
/// canonical byte image.
fn push_members(out: &mut Vec<u8>, members: &[DurableMemberShape]) {
    out.extend_from_slice(&(members.len() as u16).to_be_bytes());
    for member in members {
        match member {
            DurableMemberShape::Field(field) => {
                out.push(MEMBER_FIELD);
                push_idref(out, IDREF_FIELD, &field.id);
                out.push(u8::from(field.required));
                field.value.encode(out);
            }
            DurableMemberShape::Group(group) => {
                out.push(MEMBER_GROUP);
                push_idref(out, IDREF_GROUP, &group.id);
                push_members(out, &group.members);
            }
            DurableMemberShape::Branch(branch) => {
                out.push(MEMBER_BRANCH);
                push_idref(out, IDREF_ROOT, &branch.placement);
                push_keys(out, &branch.keys);
                push_members(out, &branch.members);
            }
        }
    }
}

/// The stable 32-byte identity of a program's durable graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DurableContractId(pub(crate) [u8; 32]);

impl DurableContractId {
    /// Reconstruct an id from its 32 raw bytes. The verifier decodes the id carried
    /// in an untrusted image with this, then compares it against the id it recomputes
    /// from the decoded graph; it never treats the carried bytes as authoritative.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The 32 identity bytes, as carried in the image DURABLE section.
    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The domain-separated, length-delimited hash construction. Kept private so the
    /// one canonical payload has a single owner; a `DurableContractDescriptor` is the
    /// only minting entry point.
    fn compute(lineage: &[u8], graph: &[u8]) -> Self {
        let mut payload: Vec<u8> = Vec::new();
        push_lp(&mut payload, lineage);
        push_lp(&mut payload, graph);
        let mut hasher = Sha256::new();
        hasher.update(DURABLE_CONTRACT_KIND);
        hasher.update((payload.len() as u64).to_be_bytes());
        hasher.update(&payload);
        DurableContractId(hasher.finalize().into())
    }

    /// The lowercase hex spelling of the identity, for diagnostics and tests.
    pub fn to_hex(self) -> String {
        let mut hex = String::with_capacity(64);
        for byte in self.0 {
            hex.push(char::from_digit(u32::from(byte >> 4), 16).expect("hex nibble"));
            hex.push(char::from_digit(u32::from(byte & 0xf), 16).expect("hex nibble"));
        }
        hex
    }
}

/// Append `u8(kind) ‖ u64_be(16) ‖ id` — a kind-tagged, length-delimited ledger id.
fn push_idref(out: &mut Vec<u8>, kind: u8, id: &LedgerIdBytes) {
    out.push(kind);
    push_lp(out, id.bytes());
}

/// Append `u64_be(len) ‖ bytes`.
fn push_lp(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    out.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::{
        DURABLE_CONTRACT_KIND, DurableBranchShape, DurableContractDescriptor,
        DurableEnumMemberShape, DurableFieldShape, DurableGroupShape, DurableKeyShape,
        DurableMemberShape, DurableRootShape, DurableValueShape, LOCAL_ROOT_LINEAGE, LedgerIdBytes,
    };
    use crate::ty::Scalar;
    use sha2::{Digest, Sha256};

    fn id(byte: u8) -> LedgerIdBytes {
        LedgerIdBytes::from_bytes([byte; 16])
    }

    fn field(byte: u8, scalar: Scalar, required: bool) -> DurableMemberShape {
        DurableMemberShape::Field(DurableFieldShape {
            id: id(byte),
            required,
            value: DurableValueShape::Scalar(scalar),
        })
    }

    /// A durable field carrying a widened value shape.
    fn value_field(byte: u8, required: bool, value: DurableValueShape) -> DurableMemberShape {
        DurableMemberShape::Field(DurableFieldShape {
            id: id(byte),
            required,
            value,
        })
    }

    /// The tracer's `counters` graph with fixed test ids: application `0x0a`,
    /// placement `0x0b`, key `0x0c`, product `0x0d`, fields `0x0e`/`0x0f`. A flat
    /// single-column-keyed resource: its member tree is two top-level fields.
    fn counters_graph() -> DurableContractDescriptor {
        DurableContractDescriptor::new(
            id(0x0a),
            vec![DurableRootShape {
                placement: id(0x0b),
                product: id(0x0d),
                keys: vec![DurableKeyShape {
                    scalar: Scalar::Text,
                    id: id(0x0c),
                }],
                members: vec![
                    field(0x0e, Scalar::Int, true),
                    field(0x0f, Scalar::Text, false),
                ],
            }],
        )
    }

    /// A richer graph exercising every member kind: a top-level field, a static
    /// `group` namespace holding a field, and a keyed `branch` placement holding a
    /// field and its own nested group. This is the shape the branch/group slice
    /// makes identity-complete.
    fn library_graph() -> DurableContractDescriptor {
        DurableContractDescriptor::new(
            id(0x0a),
            vec![DurableRootShape {
                placement: id(0x0b),
                product: id(0x0d),
                keys: vec![DurableKeyShape {
                    scalar: Scalar::Int,
                    id: id(0x0c),
                }],
                members: vec![
                    field(0x0e, Scalar::Text, true),
                    DurableMemberShape::Group(DurableGroupShape {
                        id: id(0x20),
                        members: vec![field(0x21, Scalar::Int, false)],
                    }),
                    DurableMemberShape::Branch(DurableBranchShape {
                        placement: id(0x30),
                        keys: vec![DurableKeyShape {
                            scalar: Scalar::Text,
                            id: id(0x31),
                        }],
                        members: vec![
                            field(0x32, Scalar::Text, true),
                            DurableMemberShape::Group(DurableGroupShape {
                                id: id(0x33),
                                members: vec![field(0x34, Scalar::Instant, false)],
                            }),
                        ],
                    }),
                ],
            }],
        )
    }

    #[test]
    fn kind_is_seventeen_bytes_and_distinct_from_the_other_kinds() {
        assert_eq!(DURABLE_CONTRACT_KIND.len(), 17);
        assert_ne!(
            DURABLE_CONTRACT_KIND.as_slice(),
            crate::digest::IMAGE_DIGEST_KIND.as_slice(),
        );
        assert_ne!(
            DURABLE_CONTRACT_KIND.as_slice(),
            crate::export_id::EXPORT_ID_KIND.as_slice(),
        );
    }

    /// Known-answer test for the frozen canonical payload of the tracer's `counters`
    /// graph over ledger ids. Freezing this hex pins the domain-separation,
    /// length-delimiting, IDREF kind tags, and member-tree layout so a later reader
    /// can reconstruct it independently. This value supersedes the slice-3b member-
    /// tree KAT (`344ca874…`): a durable field now records `IDREF(field) ‖ required ‖
    /// value`, the value being the closed durable value shape (here a bare scalar,
    /// `scalar(0) ‖ scalar_tag`). If this value must change, the durable-contract
    /// identity has changed and every stored/derived id changes with it.
    #[test]
    fn durable_contract_id_known_answer() {
        assert_eq!(
            counters_graph().contract_id().to_hex(),
            independent_id(&counters_graph())
        );
        // The frozen value itself.
        assert_eq!(
            counters_graph().contract_id().to_hex(),
            "0f633844944d599db964e76f209a1bc97d3785234db58e38bb478361660009bb",
        );
    }

    /// Known-answer test for a graph with a group and a keyed branch: pins the
    /// member-tag bytes (field 0, group 1, branch 2), the `Group` IDREF tag (7), and
    /// the branch placement/key-tuple layout.
    #[test]
    fn durable_contract_id_with_group_and_branch_known_answer() {
        assert_eq!(
            library_graph().contract_id().to_hex(),
            independent_id(&library_graph())
        );
        assert_eq!(
            library_graph().contract_id().to_hex(),
            "6c3cf722b607626d4dcb22edd79947521bf382166b5e93b9f1e2b509682e17b6",
        );
        assert_ne!(
            library_graph().contract_id(),
            counters_graph().contract_id()
        );
    }

    /// Independent-decoder reconstruction: a second, hand-written implementation of
    /// the construction reproduces the same 32 bytes. It shares no code with
    /// `DurableContractDescriptor::encode_graph`/`DurableContractId::compute`, so a
    /// change to the owner that silently altered the layout would diverge here.
    fn independent_id(descriptor: &DurableContractDescriptor) -> String {
        // Rebuild the graph bytes by hand from the descriptor's parts. This test
        // module is a child of the owner module, so it reads the private fields
        // directly while sharing none of the encoding code.
        fn idref(out: &mut Vec<u8>, kind: u8, id: &LedgerIdBytes) {
            out.push(kind);
            lp(out, id.bytes());
        }
        fn keys(out: &mut Vec<u8>, columns: &[DurableKeyShape]) {
            out.extend_from_slice(&(columns.len() as u16).to_be_bytes());
            for key in columns {
                out.push(key.scalar.tag());
                idref(out, 4, &key.id);
            }
        }
        fn value(out: &mut Vec<u8>, shape: &DurableValueShape) {
            match shape {
                DurableValueShape::Scalar(scalar) => {
                    out.push(0);
                    out.push(scalar.tag());
                }
                DurableValueShape::Struct(leaves) => {
                    out.push(1);
                    out.extend_from_slice(&(leaves.len() as u16).to_be_bytes());
                    for leaf in leaves {
                        value(out, leaf);
                    }
                }
                DurableValueShape::Enum { sum, members } => {
                    out.push(2);
                    idref(out, 5, sum);
                    out.extend_from_slice(&(members.len() as u16).to_be_bytes());
                    for member in members {
                        idref(out, 6, &member.id);
                        out.extend_from_slice(&(member.payload.len() as u16).to_be_bytes());
                        for leaf in &member.payload {
                            value(out, leaf);
                        }
                    }
                }
            }
        }
        fn member_tree(out: &mut Vec<u8>, members: &[DurableMemberShape]) {
            out.extend_from_slice(&(members.len() as u16).to_be_bytes());
            for member in members {
                match member {
                    DurableMemberShape::Field(f) => {
                        out.push(0);
                        idref(out, 2, &f.id);
                        out.push(u8::from(f.required));
                        value(out, &f.value);
                    }
                    DurableMemberShape::Group(g) => {
                        out.push(1);
                        idref(out, 7, &g.id);
                        member_tree(out, &g.members);
                    }
                    DurableMemberShape::Branch(b) => {
                        out.push(2);
                        idref(out, 3, &b.placement);
                        keys(out, &b.keys);
                        member_tree(out, &b.members);
                    }
                }
            }
        }
        let mut graph: Vec<u8> = Vec::new();
        graph.extend_from_slice(&(descriptor.roots.len() as u16).to_be_bytes());
        if let Some(application) = &descriptor.application {
            idref(&mut graph, 0, application);
        }
        for root in &descriptor.roots {
            idref(&mut graph, 3, &root.placement);
            idref(&mut graph, 1, &root.product);
            keys(&mut graph, &root.keys);
            member_tree(&mut graph, &root.members);
        }
        let mut payload: Vec<u8> = Vec::new();
        lp(&mut payload, LOCAL_ROOT_LINEAGE);
        lp(&mut payload, &graph);
        let mut framed: Vec<u8> = Vec::new();
        framed.extend_from_slice(DURABLE_CONTRACT_KIND);
        framed.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        framed.extend_from_slice(&payload);
        let bytes: [u8; 32] = Sha256::digest(&framed).into();
        let mut hex = String::with_capacity(64);
        for byte in bytes {
            hex.push(char::from_digit(u32::from(byte >> 4), 16).expect("hex nibble"));
            hex.push(char::from_digit(u32::from(byte & 0xf), 16).expect("hex nibble"));
        }
        hex
    }

    fn lp(out: &mut Vec<u8>, bytes: &[u8]) {
        out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        out.extend_from_slice(bytes);
    }

    /// The load-bearing D00 property: identity follows the ledger ids, not the
    /// spelling. A graph whose ids are unchanged keeps its contract id (a rename
    /// moves only the ledger anchor); a re-minted field id, a retyped key, or a
    /// flipped required flag changes it.
    #[test]
    fn identity_follows_ledger_ids_not_shape_spelling() {
        let base = counters_graph().contract_id();

        // The same ids and shape: stable (this is what a rename looks like here —
        // names are simply not part of the payload).
        assert_eq!(base, counters_graph().contract_id());

        // A re-minted top-level field id changes the id (delete-then-re-add mints
        // fresh).
        let mut re_minted = counters_graph();
        re_minted.roots[0].members[0] = field(0x1e, Scalar::Int, true);
        assert_ne!(base, re_minted.contract_id());

        // A changed key type changes the id.
        let mut rekeyed = counters_graph();
        rekeyed.roots[0].keys[0].scalar = Scalar::Int;
        assert_ne!(base, rekeyed.contract_id());

        // A re-minted key id changes the id.
        let mut rekey_id = counters_graph();
        rekey_id.roots[0].keys[0].id = id(0x2c);
        assert_ne!(base, rekey_id.contract_id());

        // An added key column (single → composite) changes the id.
        let mut composite = counters_graph();
        composite.roots[0].keys.push(DurableKeyShape {
            scalar: Scalar::Int,
            id: id(0x3c),
        });
        assert_ne!(base, composite.contract_id());

        // A field made required changes the id.
        let mut required = counters_graph();
        required.roots[0].members[1] = field(0x0f, Scalar::Text, true);
        assert_ne!(base, required.contract_id());

        // A removed field changes the id.
        let mut narrowed = counters_graph();
        narrowed.roots[0].members.pop();
        assert_ne!(base, narrowed.contract_id());

        // A different application changes the id.
        let mut other_app = counters_graph();
        other_app.application = Some(id(0x2a));
        assert_ne!(base, other_app.contract_id());
    }

    /// Group and branch structure is part of the identity, distinct from a flat
    /// field of the same ledger id.
    #[test]
    fn group_and_branch_structure_is_part_of_the_identity() {
        let base = library_graph().contract_id();
        assert_eq!(base, library_graph().contract_id());

        // Re-minting the group id changes the identity.
        let mut regrouped = library_graph();
        if let DurableMemberShape::Group(group) = &mut regrouped.roots[0].members[1] {
            group.id = id(0x2f);
        } else {
            panic!("member 1 is the group");
        }
        assert_ne!(base, regrouped.contract_id());

        // Re-minting the branch placement id changes the identity.
        let mut rebranched = library_graph();
        if let DurableMemberShape::Branch(branch) = &mut rebranched.roots[0].members[2] {
            branch.placement = id(0x3f);
        } else {
            panic!("member 2 is the branch");
        }
        assert_ne!(base, rebranched.contract_id());

        // Adding a key column to the branch changes the identity.
        let mut wider = library_graph();
        if let DurableMemberShape::Branch(branch) = &mut wider.roots[0].members[2] {
            branch.keys.push(DurableKeyShape {
                scalar: Scalar::Int,
                id: id(0x3d),
            });
        } else {
            panic!("member 2 is the branch");
        }
        assert_ne!(base, wider.contract_id());

        // Promoting the group's field to a top-level field of the same id is a
        // different graph (nesting is load-bearing), even though the field id,
        // scalar, and required flag are unchanged.
        let mut flattened = library_graph();
        flattened.roots[0].members[1] = field(0x21, Scalar::Int, false);
        assert_ne!(base, flattened.contract_id());

        // Member order is load-bearing: swapping the group and the branch changes
        // the identity.
        let mut reordered = library_graph();
        reordered.roots[0].members.swap(1, 2);
        assert_ne!(base, reordered.contract_id());
    }

    /// A graph whose resource stores widened value shapes: a dense `struct` leaf, an
    /// `Option`-shaped enum, and a user enum. Enum members carry sum (kind 5) and
    /// member (kind 6) ids; the struct records its leaves positionally with no
    /// per-leaf id.
    fn widened_graph() -> DurableContractDescriptor {
        DurableContractDescriptor::new(
            id(0x0a),
            vec![DurableRootShape {
                placement: id(0x0b),
                product: id(0x0d),
                keys: vec![DurableKeyShape {
                    scalar: Scalar::Text,
                    id: id(0x0c),
                }],
                members: vec![
                    field(0x0e, Scalar::Int, true),
                    value_field(
                        0x40,
                        false,
                        DurableValueShape::Struct(vec![
                            DurableValueShape::Scalar(Scalar::Text),
                            DurableValueShape::Scalar(Scalar::Int),
                        ]),
                    ),
                    // An Option[int]-shaped enum: none (empty) then some (int).
                    value_field(
                        0x41,
                        true,
                        DurableValueShape::Enum {
                            sum: id(0x50),
                            members: vec![
                                DurableEnumMemberShape {
                                    id: id(0x51),
                                    payload: vec![],
                                },
                                DurableEnumMemberShape {
                                    id: id(0x52),
                                    payload: vec![DurableValueShape::Scalar(Scalar::Int)],
                                },
                            ],
                        },
                    ),
                    // A user enum with three members, one carrying a payload.
                    value_field(
                        0x42,
                        false,
                        DurableValueShape::Enum {
                            sum: id(0x53),
                            members: vec![
                                DurableEnumMemberShape {
                                    id: id(0x54),
                                    payload: vec![],
                                },
                                DurableEnumMemberShape {
                                    id: id(0x55),
                                    payload: vec![],
                                },
                                DurableEnumMemberShape {
                                    id: id(0x56),
                                    payload: vec![DurableValueShape::Scalar(Scalar::Text)],
                                },
                            ],
                        },
                    ),
                ],
            }],
        )
    }

    /// Known-answer test for a durable graph with widened value shapes. Freezing
    /// this hex pins the value-shape tag bytes (scalar 0, struct 1, enum 2), the sum
    /// (5) and member (6) IDREF tags, and the payload layout.
    #[test]
    fn durable_contract_id_with_widened_values_known_answer() {
        assert_eq!(
            widened_graph().contract_id().to_hex(),
            independent_id(&widened_graph())
        );
        assert_eq!(
            widened_graph().contract_id().to_hex(),
            "85b281494717c06e47bbe63ef7d243222c5e1b37a2b30f85cd09c7c4467d43aa",
        );
        assert_ne!(
            widened_graph().contract_id(),
            counters_graph().contract_id()
        );
    }

    /// Enum member identity is part of the durable identity: a rename preserves it
    /// (ids unchanged), while re-minting a member, reordering members (append is
    /// positional), or re-typing a member payload changes it.
    #[test]
    fn enum_member_identity_follows_the_ledger_ids() {
        let base = widened_graph().contract_id();
        assert_eq!(base, widened_graph().contract_id());

        // Re-minting the sum id (a delete-then-re-add of the enum) changes the id.
        let mut re_summed = widened_graph();
        if let DurableMemberShape::Field(f) = &mut re_summed.roots[0].members[2]
            && let DurableValueShape::Enum { sum, .. } = &mut f.value
        {
            *sum = id(0x60);
        } else {
            panic!("member 2 is the Option-shaped enum field");
        }
        assert_ne!(base, re_summed.contract_id());

        // Re-minting one member id (delete-then-re-add of a variant) changes the id.
        let mut re_membered = widened_graph();
        if let DurableMemberShape::Field(f) = &mut re_membered.roots[0].members[3]
            && let DurableValueShape::Enum { members, .. } = &mut f.value
        {
            members[0].id = id(0x61);
        } else {
            panic!("member 3 is the user enum field");
        }
        assert_ne!(base, re_membered.contract_id());

        // Appending a member is positional: swapping two members changes the id, so a
        // member can never silently take another's code.
        let mut reordered = widened_graph();
        if let DurableMemberShape::Field(f) = &mut reordered.roots[0].members[3]
            && let DurableValueShape::Enum { members, .. } = &mut f.value
        {
            members.swap(0, 1);
        } else {
            panic!("member 3 is the user enum field");
        }
        assert_ne!(base, reordered.contract_id());

        // Re-typing a member payload leaf changes the id.
        let mut retyped = widened_graph();
        if let DurableMemberShape::Field(f) = &mut retyped.roots[0].members[3]
            && let DurableValueShape::Enum { members, .. } = &mut f.value
        {
            members[2].payload[0] = DurableValueShape::Scalar(Scalar::Int);
        } else {
            panic!("member 3 is the user enum field");
        }
        assert_ne!(base, retyped.contract_id());

        // Re-ordering a struct leaf changes the id (leaf order is load-bearing).
        let mut struct_swapped = widened_graph();
        if let DurableMemberShape::Field(f) = &mut struct_swapped.roots[0].members[1]
            && let DurableValueShape::Struct(leaves) = &mut f.value
        {
            leaves.swap(0, 1);
        } else {
            panic!("member 1 is the struct field");
        }
        assert_ne!(base, struct_swapped.contract_id());
    }

    /// A singleton root (empty key tuple) and a composite root (two key columns)
    /// are ordinary shapes under the length-prefixed key encoding: each agrees with
    /// the independent decoder and is distinct from the single-key graph.
    #[test]
    fn singleton_and_composite_roots_encode_and_reconstruct() {
        let singleton = DurableContractDescriptor::new(
            id(0x0a),
            vec![DurableRootShape {
                placement: id(0x0b),
                product: id(0x0d),
                keys: Vec::new(),
                members: vec![field(0x0e, Scalar::Text, true)],
            }],
        );
        assert_eq!(singleton.contract_id().to_hex(), independent_id(&singleton));

        let composite = DurableContractDescriptor::new(
            id(0x0a),
            vec![DurableRootShape {
                placement: id(0x0b),
                product: id(0x0d),
                keys: vec![
                    DurableKeyShape {
                        scalar: Scalar::Text,
                        id: id(0x0c),
                    },
                    DurableKeyShape {
                        scalar: Scalar::Int,
                        id: id(0x1c),
                    },
                ],
                members: Vec::new(),
            }],
        );
        assert_eq!(composite.contract_id().to_hex(), independent_id(&composite));
        assert_ne!(singleton.contract_id(), composite.contract_id());

        // Key-column order matters: swapping the two columns is a different graph.
        let mut swapped = composite.clone();
        swapped.roots[0].keys.swap(0, 1);
        assert_ne!(composite.contract_id(), swapped.contract_id());
    }

    #[test]
    fn the_empty_graph_has_a_stable_id() {
        let empty = DurableContractDescriptor::empty();
        assert_eq!(empty.contract_id(), empty.contract_id());
        assert_ne!(empty.contract_id(), counters_graph().contract_id());
    }

    // --- Derived semantic paths (D02): every graph node's stable ledger-id chain. ---

    use crate::semantic::{SemanticNodeKind, SemanticStepKind};

    /// The `(node kind, step kinds, step ids)` fingerprint of every semantic node in
    /// pre-order, for exact structural assertions.
    fn node_shapes(
        descriptor: &DurableContractDescriptor,
    ) -> Vec<(SemanticNodeKind, Vec<SemanticStepKind>, Vec<[u8; 16]>)> {
        descriptor
            .semantic_nodes()
            .into_iter()
            .map(|node| {
                let kinds = node.path.steps().iter().map(|s| s.kind).collect();
                let ids = node.path.steps().iter().map(|s| *s.id.bytes()).collect();
                (node.kind, kinds, ids)
            })
            .collect()
    }

    #[test]
    fn semantic_nodes_of_a_flat_root_are_the_root_and_its_fields() {
        use SemanticNodeKind::{Field, Root};
        use SemanticStepKind::{Application, Field as FieldStep, Placement};
        assert_eq!(
            node_shapes(&counters_graph()),
            vec![
                (
                    Root,
                    vec![Application, Placement],
                    vec![[0x0a; 16], [0x0b; 16]]
                ),
                (
                    Field,
                    vec![Application, Placement, FieldStep],
                    vec![[0x0a; 16], [0x0b; 16], [0x0e; 16]]
                ),
                (
                    Field,
                    vec![Application, Placement, FieldStep],
                    vec![[0x0a; 16], [0x0b; 16], [0x0f; 16]]
                ),
            ]
        );
    }

    #[test]
    fn semantic_nodes_cover_every_group_and_branch_node_in_pre_order() {
        use SemanticNodeKind::{Branch, Field, Group, Root};
        use SemanticStepKind::{Application, Field as FieldStep, Group as GroupStep, Placement};
        assert_eq!(
            node_shapes(&library_graph()),
            vec![
                (
                    Root,
                    vec![Application, Placement],
                    vec![[0x0a; 16], [0x0b; 16]]
                ),
                (
                    Field,
                    vec![Application, Placement, FieldStep],
                    vec![[0x0a; 16], [0x0b; 16], [0x0e; 16]]
                ),
                (
                    Group,
                    vec![Application, Placement, GroupStep],
                    vec![[0x0a; 16], [0x0b; 16], [0x20; 16]]
                ),
                (
                    Field,
                    vec![Application, Placement, GroupStep, FieldStep],
                    vec![[0x0a; 16], [0x0b; 16], [0x20; 16], [0x21; 16]]
                ),
                // A branch step is a Placement, like a root — a keyed node.
                (
                    Branch,
                    vec![Application, Placement, Placement],
                    vec![[0x0a; 16], [0x0b; 16], [0x30; 16]]
                ),
                (
                    Field,
                    vec![Application, Placement, Placement, FieldStep],
                    vec![[0x0a; 16], [0x0b; 16], [0x30; 16], [0x32; 16]]
                ),
                (
                    Group,
                    vec![Application, Placement, Placement, GroupStep],
                    vec![[0x0a; 16], [0x0b; 16], [0x30; 16], [0x33; 16]]
                ),
                (
                    Field,
                    vec![Application, Placement, Placement, GroupStep, FieldStep],
                    vec![[0x0a; 16], [0x0b; 16], [0x30; 16], [0x33; 16], [0x34; 16]]
                ),
            ]
        );
    }

    #[test]
    fn a_field_path_is_distinct_from_and_extends_its_container() {
        let nodes = library_graph().semantic_nodes();
        let group = nodes
            .iter()
            .find(|n| n.path.node_id() == id(0x20))
            .expect("the group node");
        let nested_field = nodes
            .iter()
            .find(|n| n.path.node_id() == id(0x21))
            .expect("the group-nested field node");
        assert_ne!(group.path, nested_field.path);
        // The field's path is exactly the group's path plus the field step.
        assert!(nested_field.path.steps().starts_with(group.path.steps()));
        assert_eq!(
            nested_field.path.steps().len(),
            group.path.steps().len() + 1
        );
    }

    #[test]
    fn re_minting_a_node_id_moves_only_paths_through_it() {
        let base = library_graph().semantic_nodes();

        // Re-mint the group id: the group node and its nested field node move to the
        // fresh id; every other node's path is untouched.
        let mut regrouped = library_graph();
        if let DurableMemberShape::Group(group) = &mut regrouped.roots[0].members[1] {
            group.id = id(0x2f);
        } else {
            panic!("member 1 is the group");
        }
        let after = regrouped.semantic_nodes();

        // The root and the top-level field keep identical paths.
        for terminal in [id(0x0b), id(0x0e), id(0x30)] {
            let before_path = base.iter().find(|n| n.path.node_id() == terminal);
            let after_path = after.iter().find(|n| n.path.node_id() == terminal);
            assert_eq!(
                before_path.map(|n| &n.path),
                after_path.map(|n| &n.path),
                "the node ending in {terminal:?} is unaffected by re-minting the group",
            );
        }
        // The group's own node now ends in the fresh id, and the old id is gone.
        assert!(after.iter().any(|n| n.path.node_id() == id(0x2f)));
        assert!(!after.iter().any(|n| n.path.node_id() == id(0x20)));
        // Its nested field's path now passes through the fresh group id.
        let nested = after
            .iter()
            .find(|n| n.path.node_id() == id(0x21))
            .expect("the nested field node");
        assert!(nested.path.steps().iter().any(|s| s.id == id(0x2f)));
    }

    #[test]
    fn the_empty_graph_has_no_semantic_nodes() {
        assert!(
            DurableContractDescriptor::empty()
                .semantic_nodes()
                .is_empty()
        );
    }
}
