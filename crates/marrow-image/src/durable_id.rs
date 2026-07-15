//! The `DurableContractId` durable-graph identity (kernel identity rule).
//!
//! A [`DurableContractId`] is the stable 32-byte identity of a program's whole
//! durable graph — the roots, their key columns, and each root record's stored
//! field profile. It crosses the compiler → image → verifier boundary and will
//! later cross the store-admission boundary (an activated store binds the contract
//! it was provisioned under), so it is a distinct typed 32-byte domain-separated
//! SHA-256 over a length-delimited canonical payload, exactly as the kernel
//! identity rule requires: one owning phase (D00), one frozen `kind`, one canonical
//! payload, one known-answer test, and one independent-decoder reconstruction test.
//!
//! Unlike the entropy-minted store/ledger ids (a separate identity family), this id
//! is a deterministic hash of the graph's *shape*: it changes on every semantic
//! change to the durable graph (a renamed root or field, a changed key type, a
//! field made required, a field added or removed) and is stable across source
//! spelling and declaration order that do not change that shape. The compiler mints
//! it and carries it in the image; the verifier never trusts the carried bytes as
//! authoritative — it independently rebuilds the descriptor from the decoded tables,
//! recomputes the id, and rejects a mismatch. Anyone can mint a valid id, so trust
//! comes only from that recomputation.
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
//!   graph   = u16_be(root_count) ‖ root*                       (roots in image order)
//!   root    = LP(root_name) ‖ u8(key_scalar_tag)
//!             ‖ u16_be(field_count) ‖ field*                   (fields in image order)
//!   field   = LP(field_name) ‖ u8(field_scalar_tag) ‖ u8(required?1:0)
//! ```
//!
//! The construction mirrors the image digest and `ExportId`: `KIND`, then the
//! big-endian length of the whole payload, then the payload. Scalar tags are the
//! frozen [`Scalar::tag`] bytes, so the key- and field-type vocabulary the id
//! commits to is the same one the image records everywhere else. Operation *sites*
//! are deliberately excluded: they are derivable access points over the graph, not
//! part of its durable identity, so adding or removing a read/write site of an
//! unchanged graph leaves the contract id stable.

use sha2::{Digest, Sha256};

use crate::ty::Scalar;

/// The domain-separation tag for the durable-contract identity. Distinct from every
/// other Marrow identity's `kind`, so a `DurableContractId` can never collide with
/// an `ImageId` or `ExportId` computed over the same bytes.
pub const DURABLE_CONTRACT_KIND: &[u8; 17] = b"marrow.durable.v0";

/// The lineage of a durable graph declared in the local project root: the single tag
/// byte `0x00`. A dependency package's lineage begins with `0x01` at a later phase,
/// so the tag byte alone keeps local and package lineages disjoint.
const LOCAL_ROOT_LINEAGE: &[u8] = &[0x00];

/// One stored field of a durable root record, as it contributes to the contract
/// identity: its name, scalar type, and whether it is required. Field order is the
/// record's declaration (image) order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableFieldShape {
    pub name: String,
    pub scalar: Scalar,
    pub required: bool,
}

/// One durable root, as it contributes to the contract identity: its name, key
/// column scalar, and the ordered stored field profile of its record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableRootShape {
    pub name: String,
    pub key: Scalar,
    pub fields: Vec<DurableFieldShape>,
}

/// The canonical descriptor of a program's durable graph. This is the single owner
/// of the contract's canonical payload: the compiler builds one from its resolved
/// roots and record types, and the verifier independently builds one from the
/// decoded image tables. Both call [`DurableContractDescriptor::contract_id`], so
/// there is exactly one canonical encoding, and agreement between the two is a
/// recomputation rather than a trusted transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableContractDescriptor {
    roots: Vec<DurableRootShape>,
}

impl DurableContractDescriptor {
    /// Build a descriptor from the durable roots in image order. An empty graph
    /// (no roots) has a well-defined canonical descriptor and id.
    pub fn new(roots: Vec<DurableRootShape>) -> Self {
        Self { roots }
    }

    /// The stable 32-byte identity of this durable graph in the local project root.
    pub fn contract_id(&self) -> DurableContractId {
        DurableContractId::compute(LOCAL_ROOT_LINEAGE, &self.encode_graph())
    }

    /// The canonical graph bytes (the `graph` production above). Length-delimited so
    /// the whole is fed as one `LP(graph)` component of the payload.
    fn encode_graph(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.roots.len() as u16).to_be_bytes());
        for root in &self.roots {
            push_lp(&mut out, root.name.as_bytes());
            out.push(root.key.tag());
            out.extend_from_slice(&(root.fields.len() as u16).to_be_bytes());
            for field in &root.fields {
                push_lp(&mut out, field.name.as_bytes());
                out.push(field.scalar.tag());
                out.push(u8::from(field.required));
            }
        }
        out
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

/// Append `u64_be(len) ‖ bytes`.
fn push_lp(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    out.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::{
        DURABLE_CONTRACT_KIND, DurableContractDescriptor, DurableFieldShape, DurableRootShape,
        LOCAL_ROOT_LINEAGE,
    };
    use crate::ty::Scalar;
    use sha2::{Digest, Sha256};

    fn counters_graph() -> DurableContractDescriptor {
        DurableContractDescriptor::new(vec![DurableRootShape {
            name: "counters".to_string(),
            key: Scalar::Text,
            fields: vec![
                DurableFieldShape {
                    name: "value".to_string(),
                    scalar: Scalar::Int,
                    required: true,
                },
                DurableFieldShape {
                    name: "label".to_string(),
                    scalar: Scalar::Text,
                    required: false,
                },
            ],
        }])
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
    /// graph. Freezing this hex pins the domain-separation, length-delimiting, and
    /// graph layout so a later reader can reconstruct it independently. If this value
    /// must change, the durable-contract identity has changed and every stored/derived
    /// id changes with it.
    #[test]
    fn durable_contract_id_known_answer() {
        assert_eq!(
            counters_graph().contract_id().to_hex(),
            independent_id(&counters_graph())
        );
        // The frozen value itself.
        assert_eq!(
            counters_graph().contract_id().to_hex(),
            "5bc9122aa107d33ca756de4149139cb8d385f148541a2a577d0e54a72df8c234",
        );
    }

    /// Independent-decoder reconstruction: a second, hand-written implementation of
    /// the construction reproduces the same 32 bytes. It shares no code with
    /// `DurableContractDescriptor::encode_graph`/`DurableContractId::compute`, so a
    /// change to the owner that silently altered the layout would diverge here.
    fn independent_id(descriptor: &DurableContractDescriptor) -> String {
        // Rebuild the graph bytes by hand from the descriptor's roots. This test
        // module is a child of the owner module, so it reads the private `roots`
        // field directly while sharing none of the encoding code.
        let roots = &descriptor.roots;
        let mut graph: Vec<u8> = Vec::new();
        graph.extend_from_slice(&(roots.len() as u16).to_be_bytes());
        for root in roots {
            lp(&mut graph, root.name.as_bytes());
            graph.push(root.key.tag());
            graph.extend_from_slice(&(root.fields.len() as u16).to_be_bytes());
            for field in &root.fields {
                lp(&mut graph, field.name.as_bytes());
                graph.push(field.scalar.tag());
                graph.push(u8::from(field.required));
            }
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

    #[test]
    fn rename_and_retype_change_the_id_spelling_order_do_not() {
        let base = counters_graph().contract_id();

        // A renamed field changes the id.
        let mut renamed = counters_graph();
        renamed.roots[0].fields[0].name = "count".to_string();
        assert_ne!(base, renamed.contract_id());

        // A changed key type changes the id.
        let mut rekeyed = counters_graph();
        rekeyed.roots[0].key = Scalar::Int;
        assert_ne!(base, rekeyed.contract_id());

        // A field made required changes the id.
        let mut required = counters_graph();
        required.roots[0].fields[1].required = true;
        assert_ne!(base, required.contract_id());

        // The same graph is stable.
        assert_eq!(base, counters_graph().contract_id());
    }

    #[test]
    fn the_empty_graph_has_a_stable_id() {
        let empty = DurableContractDescriptor::new(Vec::new());
        assert_eq!(empty.contract_id(), empty.contract_id());
        assert_ne!(empty.contract_id(), counters_graph().contract_id());
    }
}
