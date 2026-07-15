//! The deployment-ceiling descriptor and its `CeilingId` identity (E01).
//!
//! A **deployment ceiling** is the maximum durable authority a store admits: an
//! attachment resolves each invocation's effective authority as
//! `demand ∩ ceiling ∩ grant`, and the ceiling is the standing upper bound the
//! store was minted under. Its descriptor is a sorted, deduplicated set of
//! durable-access atoms — exactly the D04 atom model the verifier reconstructs for
//! [`ExportDemand`] — so a deployment's ceiling is the program-wide demand *union*
//! it chooses to admit. The compiler describes demand; a deployment picks a ceiling;
//! neither grants — the kernel intersects the two with the invocation grant.
//!
//! The [`CeilingId`] is a stable-boundary hash identity exactly as the kernel
//! identity rule requires — a domain-separated SHA-256 over the same
//! length-delimited canonical atom-set payload [`ExportDemand`] owns, under a
//! *distinct* frozen `kind`:
//!
//! ```text
//! CeilingId = SHA-256( KIND ‖ u64_be(len(payload)) ‖ payload )
//!   KIND    = b"marrow.ceilng.v0"
//!   payload = LP(lineage) ‖ u32_be(atom_count) ‖ atom*   (ascending by atom bytes, deduplicated)
//! ```
//!
//! The atom-set payload has one owner ([`ExportDemand::atom_set_payload`]); the
//! demand identity and the ceiling identity differ only in their `kind`. Domain
//! separation is load-bearing: a `DemandSetId` and a `CeilingId` computed over the
//! *same* atom set are different bytes, so a demand identity can never be presented
//! as a ceiling identity that would widen authority. The attachment mint binds the
//! ceiling id, so the store carries the exact ceiling it was minted under.

use crate::demand::{ExportDemand, frame_id};

/// The domain-separation tag for the deployment-ceiling identity. Distinct from every
/// other Marrow identity's `kind`, so a `CeilingId` can never collide with a
/// `DemandSetId`, `ImageId`, `ExportId`, or `DurableContractId` computed over the
/// same bytes.
pub const CEILING_KIND: &[u8; 16] = b"marrow.ceilng.v0";

/// The descriptor of a deployment ceiling: the sorted, deduplicated atom set the
/// store admits, as a canonical [`ExportDemand`]. Built only from a demand union, so
/// the canonical order and deduplication are established by [`ExportDemand`]'s one
/// owner. An upper bound on authority, never a grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CeilingDescriptor {
    atoms: ExportDemand,
}

impl CeilingDescriptor {
    /// The ceiling that admits exactly a program's demand union. A deployment's
    /// maximum durable authority is the union of what its exports demand; a narrower
    /// or wider ceiling is a different descriptor with a different [`CeilingId`].
    pub fn from_demand_union(union: ExportDemand) -> Self {
        Self { atoms: union }
    }

    /// The canonical atom set this ceiling admits.
    pub fn atoms(&self) -> &ExportDemand {
        &self.atoms
    }

    /// Whether the ceiling admits any observing access (read, presence, or
    /// traversal) — the `read` term of the read/write coverage the store checks.
    pub fn reads(&self) -> bool {
        self.atoms.reads()
    }

    /// Whether the ceiling admits any mutating access (write or erase) — the `write`
    /// term of the read/write coverage the store checks.
    pub fn writes(&self) -> bool {
        self.atoms.writes()
    }

    /// The stable 32-byte identity of this ceiling: the atom-set payload framed under
    /// [`CEILING_KIND`].
    pub fn ceiling_id(&self) -> CeilingId {
        CeilingId(frame_id(CEILING_KIND, &self.atoms.atom_set_payload()))
    }
}

/// The stable 32-byte identity of a deployment ceiling. Separate from the
/// [`DemandSetId`](crate::DemandSetId) over the same atoms (a different `kind`), so a
/// demand identity and a ceiling identity are never interchangeable. The attachment
/// mint records this id, binding a store to the exact ceiling it was minted under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CeilingId([u8; 32]);

impl CeilingId {
    /// Reconstruct an id from its 32 raw bytes, for a downstream consumer (the path
    /// kernel) that carries the id as an opaque binding token without recomputing it.
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

#[cfg(test)]
mod tests {
    use super::{CEILING_KIND, CeilingDescriptor, CeilingId};
    use crate::demand::{DemandAtom, ExportDemand, OperationClass};
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

    /// A two-atom demand union: read the whole entry, write a field. Fixed ids so the
    /// KAT is reproducible — deliberately the same sample as the demand-identity KAT,
    /// so the domain-separation assertion is exact.
    fn sample_union() -> ExportDemand {
        ExportDemand::from_atoms([
            DemandAtom::new(root_path(), OperationClass::Read),
            DemandAtom::new(field_path(0x0e), OperationClass::Write),
        ])
    }

    #[test]
    fn kind_is_sixteen_bytes_and_distinct_from_the_other_kinds() {
        assert_eq!(CEILING_KIND.len(), 16);
        for other in [
            crate::demand::DEMAND_SET_KIND.as_slice(),
            crate::export_id::EXPORT_ID_KIND.as_slice(),
            crate::durable_id::DURABLE_CONTRACT_KIND.as_slice(),
            crate::digest::IMAGE_DIGEST_KIND.as_slice(),
        ] {
            assert_ne!(CEILING_KIND.as_slice(), other);
        }
    }

    /// Known-answer test for the frozen canonical payload of the sample ceiling.
    /// Freezing this hex pins the domain-separation kind, the shared atom-set payload,
    /// and the framing. If this value must change, the ceiling identity contract has
    /// changed.
    #[test]
    fn ceiling_id_known_answer() {
        let ceiling = CeilingDescriptor::from_demand_union(sample_union());
        assert_eq!(
            ceiling.ceiling_id().to_hex(),
            independent_id(&sample_union())
        );
        assert_eq!(
            ceiling.ceiling_id().to_hex(),
            "d088e9ea2a0b6d5e4559a055f748b14cab6a4a4339928aea8bb4c93f9f724f32",
        );
    }

    /// Independent-decoder reconstruction: a second, hand-written implementation of
    /// the construction reproduces the same 32 bytes, sharing no code with
    /// `CeilingDescriptor::ceiling_id` or `ExportDemand::atom_set_payload`.
    fn independent_id(union: &ExportDemand) -> String {
        let mut bodies: Vec<Vec<u8>> = union
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
        // LP(lineage): the local project root is the single tag byte 0x00.
        payload.extend_from_slice(&1u64.to_be_bytes());
        payload.push(0x00);
        payload.extend_from_slice(&(bodies.len() as u32).to_be_bytes());
        for body in &bodies {
            payload.extend_from_slice(&(body.len() as u64).to_be_bytes());
            payload.extend_from_slice(body);
        }
        let mut framed: Vec<u8> = Vec::new();
        framed.extend_from_slice(CEILING_KIND);
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

    /// Domain separation is load-bearing: the ceiling id and the demand id computed
    /// over the *same* atom set are different bytes, so a demand identity can never be
    /// presented as a ceiling identity.
    #[test]
    fn ceiling_id_differs_from_the_demand_id_over_the_same_atoms() {
        let union = sample_union();
        let ceiling = CeilingDescriptor::from_demand_union(union.clone());
        assert_ne!(
            ceiling.ceiling_id().bytes(),
            union.demand_set_id().bytes(),
            "same atoms must frame to distinct ids under distinct kinds",
        );
    }

    /// The identity is over the *set*: discovery order and duplicates do not change
    /// it, but any atom difference does.
    #[test]
    fn ceiling_id_is_over_the_atom_set() {
        let base = CeilingDescriptor::from_demand_union(sample_union()).ceiling_id();

        // Reversed discovery order with a duplicate: same canonical set, same id.
        let shuffled = ExportDemand::from_atoms([
            DemandAtom::new(field_path(0x0e), OperationClass::Write),
            DemandAtom::new(root_path(), OperationClass::Read),
            DemandAtom::new(field_path(0x0e), OperationClass::Write),
        ]);
        assert_eq!(
            base,
            CeilingDescriptor::from_demand_union(shuffled).ceiling_id()
        );

        // A wider ceiling (an added atom) is a different identity — so a store minted
        // under a narrow ceiling cannot be re-presented under a wider one.
        let wider = ExportDemand::from_atoms([
            DemandAtom::new(root_path(), OperationClass::Read),
            DemandAtom::new(field_path(0x0e), OperationClass::Write),
            DemandAtom::new(field_path(0x0e), OperationClass::Erase),
        ]);
        assert_ne!(
            base,
            CeilingDescriptor::from_demand_union(wider).ceiling_id()
        );
    }

    /// Read/write coverage projects the atom classes, so the store ceiling's
    /// read/write check and the ceiling descriptor agree.
    #[test]
    fn coverage_projects_the_classes() {
        let read_only =
            CeilingDescriptor::from_demand_union(ExportDemand::from_atoms([DemandAtom::new(
                root_path(),
                OperationClass::Read,
            )]));
        assert!(read_only.reads());
        assert!(!read_only.writes());

        let writing =
            CeilingDescriptor::from_demand_union(ExportDemand::from_atoms([DemandAtom::new(
                field_path(0x0e),
                OperationClass::Write,
            )]));
        assert!(writing.writes());
    }

    /// The empty ceiling (a storeless deployment) has a stable id distinct from any
    /// nonempty ceiling.
    #[test]
    fn the_empty_ceiling_has_a_stable_id() {
        let empty = CeilingDescriptor::from_demand_union(ExportDemand::from_atoms([]));
        assert_eq!(empty.ceiling_id(), empty.ceiling_id());
        assert_ne!(
            empty.ceiling_id(),
            CeilingDescriptor::from_demand_union(sample_union()).ceiling_id()
        );
    }

    #[test]
    fn ceiling_id_round_trips_through_bytes() {
        let ceiling = CeilingDescriptor::from_demand_union(sample_union());
        let id = ceiling.ceiling_id();
        assert_eq!(CeilingId::from_bytes(*id.bytes()), id);
    }
}
