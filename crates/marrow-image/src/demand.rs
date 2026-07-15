//! Verifier-reconstructed durable demand and the `DemandSetId` identity (D04).
//!
//! An export's **demand** is the stable set of durable-access atoms its whole call
//! closure performs: for every durable operation the export reaches, *which* graph
//! node it touches (a [`SemanticPath`]) and *what class* of access it makes (a
//! closed [`OperationClass`]). Demand describes access; it never grants it. The
//! kernel identity rule (`AGENTS.md`): "The compiler describes access demand; it
//! never grants what it infers." Runtime authority is `demand ∩ ceiling ∩ grant`
//! resolved at the path kernel; this owner supplies only the `demand` term.
//!
//! Demand is a **compiler fact reconstructed by the verifier**, not a serialized
//! summary: the image carries operation *sites* (a path + target) and bytecode, and
//! the verifier rebuilds each export's atom set from the sealed sites its call
//! closure references. Nothing about demand — no atom, no incidence, no consequence
//! byte — is written into the image. Two representations stay distinct: an
//! export's stable [`ExportDemand`] (its atom set, identified by [`DemandSetId`])
//! and its image-local reachable-site set (site indices, meaningful only within one
//! [`ImageId`](crate::ImageId)); a body edit that leaves the atom set unchanged
//! keeps the `DemandSetId` while changing the `ImageId`.
//!
//! The [`DemandSetId`] is a stable-boundary hash identity exactly as the identity
//! rule requires — a domain-separated SHA-256 over a length-delimited canonical
//! payload with one frozen `kind`, one canonical payload, one known-answer test,
//! and one independent-decoder reconstruction test:
//!
//! ```text
//! DemandSetId = SHA-256( KIND ‖ u64_be(len(payload)) ‖ payload )
//!   KIND      = b"marrow.demand.v0"
//!   payload   = LP(lineage) ‖ u32_be(atom_count) ‖ atom*   (ascending by atom bytes, deduplicated)
//!   atom      = LP(atom_body)
//!   atom_body = u8(class_tag) ‖ u16_be(step_count) ‖ step*
//!   step      = u8(step_kind_ledger_byte) ‖ id16          (a semantic path step)
//!   LP(b)     = u64_be(b.len()) ‖ b
//!   lineage   = the demand's package lineage. The local project root is the single
//!               tag byte 0x00; a dependency package is 0x01 ‖ <32-byte package id>
//!               at a later phase, mirroring the `ExportId`/`DurableContractId`
//!               lineage seam.
//!   class_tag = read 0, write 1, presence 2, erase 3, index_read 4
//! ```
//!
//! Canonical order is ascending lexicographic over each atom's `atom_body` bytes,
//! deduplicated, so two demands are equal iff their atom sets are equal regardless
//! of the order the verifier discovered them. Because the atoms carry entropy-minted
//! ledger ids, moving an atom from one export to another leaves the program-wide
//! union's `DemandSetId` unchanged while changing both exports' ids.

use sha2::{Digest, Sha256};

use crate::semantic::SemanticPath;

/// The domain-separation tag for the demand-set identity. Distinct from every other
/// Marrow identity's `kind`, so a `DemandSetId` can never collide with an `ImageId`,
/// `ExportId`, or `DurableContractId` computed over the same bytes.
pub const DEMAND_SET_KIND: &[u8; 16] = b"marrow.demand.v0";

/// The lineage of demand computed in the local project root: the single tag byte
/// `0x00`. A dependency package's lineage begins with `0x01` at a later phase.
const LOCAL_ROOT_LINEAGE: &[u8] = &[0x00];

/// The closed class of durable access one atom records, projected from the durable
/// operation algebra. `create`, `replace`, and a required/sparse field set all
/// stage a [`Write`](Self::Write) at the addressed node — the authority-relevant
/// fact is *mutate in place*, not the algebra's finer created-vs-replaced runtime
/// outcome. [`Erase`](Self::Erase) is separated from `Write` because destroying a
/// node is a distinct authority from writing one. [`Presence`](Self::Presence) is
/// the existence probe, [`Read`](Self::Read) observes a value, and
/// [`IndexRead`](Self::IndexRead) is ordered key traversal — each a distinct
/// authority atom so a later path-granular ceiling can permit reads without writes,
/// presence without reads, or traversal without either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OperationClass {
    Read,
    Write,
    Presence,
    Erase,
    IndexRead,
}

impl OperationClass {
    /// The frozen canonical tag byte for this class, load-bearing in the
    /// `DemandSetId` payload.
    pub fn tag(self) -> u8 {
        match self {
            OperationClass::Read => 0,
            OperationClass::Write => 1,
            OperationClass::Presence => 2,
            OperationClass::Erase => 3,
            OperationClass::IndexRead => 4,
        }
    }

    /// The class for a canonical tag byte, or `None` for a byte outside the closed
    /// class set. The inverse of [`Self::tag`], for an independent decoder.
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(OperationClass::Read),
            1 => Some(OperationClass::Write),
            2 => Some(OperationClass::Presence),
            3 => Some(OperationClass::Erase),
            4 => Some(OperationClass::IndexRead),
            _ => None,
        }
    }

    /// Whether this class mutates durable state ([`Write`](Self::Write) or
    /// [`Erase`](Self::Erase)). The read/write coverage the store ceiling checks
    /// today is derived from this: an export "writes" iff any atom mutates.
    pub fn mutates(self) -> bool {
        matches!(self, OperationClass::Write | OperationClass::Erase)
    }

    /// The lowercase source-vocabulary word an authority review renders this class
    /// as — a path plus an operation word, with no lifecycle vocabulary.
    pub fn word(self) -> &'static str {
        match self {
            OperationClass::Read => "read",
            OperationClass::Write => "write",
            OperationClass::Presence => "presence",
            OperationClass::Erase => "erase",
            OperationClass::IndexRead => "iterate",
        }
    }
}

/// One durable-access atom: the graph node a [`SemanticPath`] names and the
/// [`OperationClass`] of access made at it. The path is the stable ledger-id chain
/// (never a container index or source spelling), so an atom is a stable fact that a
/// rename preserves and a re-mint moves.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DemandAtom {
    path: SemanticPath,
    class: OperationClass,
}

impl DemandAtom {
    pub fn new(path: SemanticPath, class: OperationClass) -> Self {
        Self { path, class }
    }

    pub fn path(&self) -> &SemanticPath {
        &self.path
    }

    pub fn class(&self) -> OperationClass {
        self.class
    }

    /// The canonical `atom_body` bytes: `u8(class_tag) ‖ u16_be(step_count) ‖ step*`.
    /// The sole owner of an atom's byte spelling; both the identity payload and the
    /// canonical-order sort key project from it.
    fn encode_body(&self) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(self.class.tag());
        let steps = self.path.steps();
        body.extend_from_slice(&(steps.len() as u16).to_be_bytes());
        for step in steps {
            body.push(step.kind.ledger_kind());
            body.extend_from_slice(step.id.bytes());
        }
        body
    }
}

/// The verifier-reconstructed durable demand of one export (or the program-wide
/// union): its canonical set of [`DemandAtom`]s, sorted ascending by atom bytes and
/// deduplicated. Built only by [`Self::from_atoms`], so the canonical order and
/// deduplication are established once. An input to the authority check, never a
/// grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportDemand {
    /// Canonical: ascending by `encode_body`, deduplicated.
    atoms: Vec<DemandAtom>,
}

impl ExportDemand {
    /// Build a canonical demand from any atom iterator: sort ascending by canonical
    /// atom bytes and drop duplicates. Discovery order does not matter, so the same
    /// atom set always yields the same canonical demand and the same
    /// [`DemandSetId`].
    pub fn from_atoms(atoms: impl IntoIterator<Item = DemandAtom>) -> Self {
        let mut keyed: Vec<(Vec<u8>, DemandAtom)> = atoms
            .into_iter()
            .map(|atom| (atom.encode_body(), atom))
            .collect();
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        keyed.dedup_by(|a, b| a.0 == b.0);
        Self {
            atoms: keyed.into_iter().map(|(_, atom)| atom).collect(),
        }
    }

    /// The canonical, sorted, deduplicated atoms.
    pub fn atoms(&self) -> &[DemandAtom] {
        &self.atoms
    }

    /// Whether this demand touches the store at all.
    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty()
    }

    /// Whether any atom observes durable state without mutating it (read, presence,
    /// or traversal) — the `read` term of the store ceiling's read/write coverage.
    pub fn reads(&self) -> bool {
        self.atoms.iter().any(|atom| !atom.class.mutates())
    }

    /// Whether any atom mutates durable state (write or erase) — the `write` term of
    /// the store ceiling's read/write coverage.
    pub fn writes(&self) -> bool {
        self.atoms.iter().any(|atom| atom.class.mutates())
    }

    /// The program-wide union of several demands: the canonical demand over every
    /// atom of every input. The deployment ceiling admits a store by the union; one
    /// invocation checks its own export's named demand.
    pub fn union<'a>(demands: impl IntoIterator<Item = &'a ExportDemand>) -> Self {
        Self::from_atoms(
            demands
                .into_iter()
                .flat_map(|demand| demand.atoms.iter().cloned()),
        )
    }

    /// The canonical atom-set payload: `LP(lineage) ‖ u32_be(atom_count) ‖ atom*`
    /// over the sorted, deduplicated atoms. This is the single owner of the atom-set
    /// byte spelling; both the [`DemandSetId`] and the deployment
    /// [`CeilingId`](crate::CeilingId) frame it under their own domain-separation
    /// `kind`, so the two identities can never collide over the same atom set.
    pub(crate) fn atom_set_payload(&self) -> Vec<u8> {
        let mut payload: Vec<u8> = Vec::new();
        push_lp(&mut payload, LOCAL_ROOT_LINEAGE);
        payload.extend_from_slice(&(self.atoms.len() as u32).to_be_bytes());
        for atom in &self.atoms {
            push_lp(&mut payload, &atom.encode_body());
        }
        payload
    }

    /// The stable 32-byte identity of this demand set.
    pub fn demand_set_id(&self) -> DemandSetId {
        DemandSetId(frame_id(DEMAND_SET_KIND, &self.atom_set_payload()))
    }
}

/// The domain-separated, length-delimited identity framing shared by every atom-set
/// identity: `SHA-256( kind ‖ u64_be(len(payload)) ‖ payload )`. The `kind` supplies
/// domain separation, so the same payload frames to distinct ids under distinct
/// kinds.
pub(crate) fn frame_id(kind: &[u8], payload: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(kind);
    hasher.update((payload.len() as u64).to_be_bytes());
    hasher.update(payload);
    hasher.finalize().into()
}

/// The stable 32-byte identity of an export's demand set. Separate from the export's
/// [`ExportId`](crate::ExportId) (its declaration identity) and the image's
/// [`ImageId`](crate::ImageId) (its byte digest): a body edit that preserves the
/// atom set preserves this id while changing the `ImageId`, and moving an atom
/// between exports changes both exports' ids while leaving the program union's id
/// unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DemandSetId([u8; 32]);

impl DemandSetId {
    /// Reconstruct an id from its 32 raw bytes. There is no trusted serialized
    /// demand id to decode — the verifier always recomputes demand from the sealed
    /// sites — so this exists only for a downstream fact consumer that stores an id
    /// alongside its own data.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The 32 identity bytes.
    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
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
        DEMAND_SET_KIND, DemandAtom, ExportDemand, LOCAL_ROOT_LINEAGE, OperationClass, push_lp,
    };
    use crate::durable_id::LedgerIdBytes;
    use crate::semantic::{SemanticPath, SemanticStep, SemanticStepKind};
    use sha2::{Digest, Sha256};

    fn id(byte: u8) -> LedgerIdBytes {
        LedgerIdBytes::from_bytes([byte; 16])
    }

    /// The path `[application 0x0a, placement 0x0b]` — a whole-entry root site.
    fn root_path() -> SemanticPath {
        SemanticPath::from_steps(vec![
            SemanticStep::new(SemanticStepKind::Application, id(0x0a)),
            SemanticStep::new(SemanticStepKind::Placement, id(0x0b)),
        ])
    }

    /// The path `[application 0x0a, placement 0x0b, field 0x0e]` — a field leaf.
    fn field_path(field: u8) -> SemanticPath {
        SemanticPath::from_steps(vec![
            SemanticStep::new(SemanticStepKind::Application, id(0x0a)),
            SemanticStep::new(SemanticStepKind::Placement, id(0x0b)),
            SemanticStep::new(SemanticStepKind::Field, id(field)),
        ])
    }

    /// A two-atom demand: read the whole entry, write a field. Fixed ids so the KAT
    /// is reproducible.
    fn sample_demand() -> ExportDemand {
        ExportDemand::from_atoms([
            DemandAtom::new(root_path(), OperationClass::Read),
            DemandAtom::new(field_path(0x0e), OperationClass::Write),
        ])
    }

    #[test]
    fn kind_is_sixteen_bytes_and_distinct_from_the_other_kinds() {
        assert_eq!(DEMAND_SET_KIND.len(), 16);
        for other in [
            crate::export_id::EXPORT_ID_KIND.as_slice(),
            crate::durable_id::DURABLE_CONTRACT_KIND.as_slice(),
            crate::digest::IMAGE_DIGEST_KIND.as_slice(),
        ] {
            assert_ne!(DEMAND_SET_KIND.as_slice(), other);
        }
    }

    /// Known-answer test for the frozen canonical payload of the sample demand.
    /// Freezing this hex pins the domain-separation, length-delimiting, class tags,
    /// and step layout so a later reader can reconstruct it independently. If this
    /// value must change, the demand identity contract has changed.
    #[test]
    fn demand_set_id_known_answer() {
        assert_eq!(
            sample_demand().demand_set_id().to_hex(),
            independent_id(&sample_demand()),
        );
        assert_eq!(
            sample_demand().demand_set_id().to_hex(),
            "8566dfdb7c3aa959e58c6da93ad6543ea017f7dbf2b59c3ba1b6855c52f0c017",
        );
    }

    /// Independent-decoder reconstruction: a second, hand-written implementation of
    /// the construction reproduces the same 32 bytes, sharing no code with
    /// `ExportDemand::demand_set_id`.
    fn independent_id(demand: &ExportDemand) -> String {
        // Rebuild each atom body, sort ascending by bytes, dedup, then frame.
        let mut bodies: Vec<Vec<u8>> = demand
            .atoms()
            .iter()
            .map(|atom| {
                let mut body = Vec::new();
                body.push(match atom.class() {
                    OperationClass::Read => 0u8,
                    OperationClass::Write => 1,
                    OperationClass::Presence => 2,
                    OperationClass::Erase => 3,
                    OperationClass::IndexRead => 4,
                });
                let steps = atom.path().steps();
                body.extend_from_slice(&(steps.len() as u16).to_be_bytes());
                for step in steps {
                    body.push(step.kind.ledger_kind());
                    body.extend_from_slice(step.id.bytes());
                }
                body
            })
            .collect();
        bodies.sort();
        bodies.dedup();

        let mut payload: Vec<u8> = Vec::new();
        push_lp(&mut payload, LOCAL_ROOT_LINEAGE);
        payload.extend_from_slice(&(bodies.len() as u32).to_be_bytes());
        for body in &bodies {
            push_lp(&mut payload, body);
        }
        let mut framed: Vec<u8> = Vec::new();
        framed.extend_from_slice(DEMAND_SET_KIND);
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

    /// The identity is over the *set*: discovery order and duplicates do not change
    /// it, but any atom difference does.
    #[test]
    fn demand_id_is_over_the_atom_set() {
        let base = sample_demand().demand_set_id();

        // Reversed discovery order with a duplicate: same canonical set, same id.
        let shuffled = ExportDemand::from_atoms([
            DemandAtom::new(field_path(0x0e), OperationClass::Write),
            DemandAtom::new(root_path(), OperationClass::Read),
            DemandAtom::new(field_path(0x0e), OperationClass::Write),
        ]);
        assert_eq!(base, shuffled.demand_set_id());

        // A different class at the same path changes the id.
        let reclassed = ExportDemand::from_atoms([
            DemandAtom::new(root_path(), OperationClass::Write),
            DemandAtom::new(field_path(0x0e), OperationClass::Write),
        ]);
        assert_ne!(base, reclassed.demand_set_id());

        // A different path changes the id.
        let repathed = ExportDemand::from_atoms([
            DemandAtom::new(root_path(), OperationClass::Read),
            DemandAtom::new(field_path(0x0f), OperationClass::Write),
        ]);
        assert_ne!(base, repathed.demand_set_id());

        // An added atom changes the id.
        let widened = ExportDemand::from_atoms([
            DemandAtom::new(root_path(), OperationClass::Read),
            DemandAtom::new(field_path(0x0e), OperationClass::Write),
            DemandAtom::new(field_path(0x0e), OperationClass::Presence),
        ]);
        assert_ne!(base, widened.demand_set_id());
    }

    /// The empty demand has a stable id distinct from any nonempty demand.
    #[test]
    fn the_empty_demand_has_a_stable_id() {
        let empty = ExportDemand::from_atoms([]);
        assert!(empty.is_empty());
        assert_eq!(
            empty.demand_set_id(),
            ExportDemand::from_atoms([]).demand_set_id()
        );
        assert_ne!(empty.demand_set_id(), sample_demand().demand_set_id());
    }

    /// Read/write coverage is the projection the store ceiling checks: presence,
    /// read, and traversal are reads; write and erase are writes.
    #[test]
    fn coverage_projects_the_classes() {
        let read_only = ExportDemand::from_atoms([
            DemandAtom::new(root_path(), OperationClass::Presence),
            DemandAtom::new(field_path(0x0e), OperationClass::Read),
            DemandAtom::new(root_path(), OperationClass::IndexRead),
        ]);
        assert!(read_only.reads());
        assert!(!read_only.writes());

        let erase_only =
            ExportDemand::from_atoms([DemandAtom::new(root_path(), OperationClass::Erase)]);
        assert!(!erase_only.reads());
        assert!(erase_only.writes());
    }

    /// Moving an atom between two exports changes each export's id but not the union.
    #[test]
    fn moving_an_atom_between_exports_preserves_the_union() {
        let read_atom = DemandAtom::new(root_path(), OperationClass::Read);
        let write_atom = DemandAtom::new(field_path(0x0e), OperationClass::Write);

        // Layout A: export one reads, export two writes.
        let a1 = ExportDemand::from_atoms([read_atom.clone()]);
        let a2 = ExportDemand::from_atoms([write_atom.clone()]);
        // Layout B: export one does both, export two does nothing.
        let b1 = ExportDemand::from_atoms([read_atom.clone(), write_atom.clone()]);
        let b2 = ExportDemand::from_atoms([]);

        assert_ne!(a1.demand_set_id(), b1.demand_set_id());
        assert_ne!(a2.demand_set_id(), b2.demand_set_id());
        assert_eq!(
            ExportDemand::union([&a1, &a2]).demand_set_id(),
            ExportDemand::union([&b1, &b2]).demand_set_id(),
        );
    }
}
