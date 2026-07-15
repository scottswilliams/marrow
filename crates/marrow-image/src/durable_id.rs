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
//!             ‖ u8(key_scalar_tag) ‖ IDREF(0x04, key)
//!             ‖ u16_be(field_count) ‖ field*                   (fields in image order)
//!   field   = IDREF(0x02, field) ‖ u8(field_scalar_tag) ‖ u8(required?1:0)
//!   IDREF(k, id) = u8(k) ‖ u64_be(16) ‖ id                     (kind-tagged, LP 16 bytes)
//! ```
//!
//! The `IDREF` kind tags mirror the ledger's frozen kind space (application 0,
//! product 1, field 2, root placement 3, key 4; 5-7 reserved). An empty graph
//! (no roots) has no application component: a storeless project needs no ledger,
//! so its contract commits to nothing. Scalar tags are the frozen
//! [`Scalar::tag`] bytes. Operation *sites* are deliberately excluded: they are
//! derivable access points over the graph, not part of its durable identity.

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

/// The frozen `IDREF` kind tags, mirroring the ledger's kind space.
const IDREF_APPLICATION: u8 = 0;
const IDREF_PRODUCT: u8 = 1;
const IDREF_FIELD: u8 = 2;
const IDREF_ROOT: u8 = 3;
const IDREF_KEY: u8 = 4;

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

/// One stored field of a durable root record, as it contributes to the contract
/// identity: its ledger id, scalar type, and whether it is required. Field order is
/// the record's declaration (image) order. The field's *name* is not part of the
/// identity — a rename preserves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableFieldShape {
    pub id: LedgerIdBytes,
    pub scalar: Scalar,
    pub required: bool,
}

/// One durable root, as it contributes to the contract identity: its placement id,
/// the stored product's id, the key column's scalar and id, and the ordered stored
/// field profile of its record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableRootShape {
    pub placement: LedgerIdBytes,
    pub product: LedgerIdBytes,
    pub key: Scalar,
    pub key_id: LedgerIdBytes,
    pub fields: Vec<DurableFieldShape>,
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
            out.push(root.key.tag());
            push_idref(&mut out, IDREF_KEY, &root.key_id);
            out.extend_from_slice(&(root.fields.len() as u16).to_be_bytes());
            for field in &root.fields {
                push_idref(&mut out, IDREF_FIELD, &field.id);
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
        DURABLE_CONTRACT_KIND, DurableContractDescriptor, DurableFieldShape, DurableRootShape,
        LOCAL_ROOT_LINEAGE, LedgerIdBytes,
    };
    use crate::ty::Scalar;
    use sha2::{Digest, Sha256};

    fn id(byte: u8) -> LedgerIdBytes {
        LedgerIdBytes::from_bytes([byte; 16])
    }

    /// The tracer's `counters` graph with fixed test ids: application `0x0a`,
    /// placement `0x0b`, key `0x0c`, product `0x0d`, fields `0x0e`/`0x0f`.
    fn counters_graph() -> DurableContractDescriptor {
        DurableContractDescriptor::new(
            id(0x0a),
            vec![DurableRootShape {
                placement: id(0x0b),
                product: id(0x0d),
                key: Scalar::Text,
                key_id: id(0x0c),
                fields: vec![
                    DurableFieldShape {
                        id: id(0x0e),
                        scalar: Scalar::Int,
                        required: true,
                    },
                    DurableFieldShape {
                        id: id(0x0f),
                        scalar: Scalar::Text,
                        required: false,
                    },
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
    /// length-delimiting, IDREF kind tags, and graph layout so a later reader can
    /// reconstruct it independently. This value supersedes the slice-1 name-based
    /// KAT (`5bc9122a…`): the payload now carries ledger ids in place of names, so
    /// a rename preserves the identity. If this value must change, the
    /// durable-contract identity has changed and every stored/derived id changes
    /// with it.
    #[test]
    fn durable_contract_id_known_answer() {
        assert_eq!(
            counters_graph().contract_id().to_hex(),
            independent_id(&counters_graph())
        );
        // The frozen value itself.
        assert_eq!(
            counters_graph().contract_id().to_hex(),
            "8f1d8fc8addf6d274f82f5a4e9d25223d7ab851a39889e394f736ac1942b129f",
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
        let mut graph: Vec<u8> = Vec::new();
        graph.extend_from_slice(&(descriptor.roots.len() as u16).to_be_bytes());
        if let Some(application) = &descriptor.application {
            idref(&mut graph, 0, application);
        }
        for root in &descriptor.roots {
            idref(&mut graph, 3, &root.placement);
            idref(&mut graph, 1, &root.product);
            graph.push(root.key.tag());
            idref(&mut graph, 4, &root.key_id);
            graph.extend_from_slice(&(root.fields.len() as u16).to_be_bytes());
            for field in &root.fields {
                idref(&mut graph, 2, &field.id);
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

        // A re-minted field id changes the id (delete-then-re-add mints fresh).
        let mut re_minted = counters_graph();
        re_minted.roots[0].fields[0].id = id(0x1e);
        assert_ne!(base, re_minted.contract_id());

        // A changed key type changes the id.
        let mut rekeyed = counters_graph();
        rekeyed.roots[0].key = Scalar::Int;
        assert_ne!(base, rekeyed.contract_id());

        // A field made required changes the id.
        let mut required = counters_graph();
        required.roots[0].fields[1].required = true;
        assert_ne!(base, required.contract_id());

        // A removed field changes the id.
        let mut narrowed = counters_graph();
        narrowed.roots[0].fields.pop();
        assert_ne!(base, narrowed.contract_id());

        // A different application changes the id.
        let mut other_app = counters_graph();
        other_app.application = Some(id(0x2a));
        assert_ne!(base, other_app.contract_id());
    }

    #[test]
    fn the_empty_graph_has_a_stable_id() {
        let empty = DurableContractDescriptor::empty();
        assert_eq!(empty.contract_id(), empty.contract_id());
        assert_ne!(empty.contract_id(), counters_graph().contract_id());
    }
}
