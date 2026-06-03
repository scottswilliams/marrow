//! The read-only evolution witness: the exact artifact a future apply consumes.
//!
//! The witness invents no identity model. It composes fingerprints that already
//! exist: the analyzed-source digest, the accepted catalog epoch/digest the
//! snapshot was computed against, the catalog proposal epoch/digest to activate,
//! the store engine-profile digest and layout epoch, the latest commit id, the
//! catalog ids the change touches, and the per-obligation verdicts the discharge
//! produced. Apply re-reads the store fingerprints and aborts on any drift before
//! staging a write, so every field here is a thing apply must re-verify.
//!
//! The verdicts that cross into apply are prose-free: each names a catalog id and
//! the typed outcome apply re-verifies. Human place labels stay on the discharge
//! result for preview and diagnostics; they never enter the witness.

use marrow_store::cell::CatalogId;
use marrow_store::tree::EngineProfileDigest;
use marrow_store::value::ScalarType;

/// One catalog snapshot fingerprint: the epoch it was minted at and its digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogFingerprint {
    pub epoch: u64,
    pub digest: String,
}

/// The counts a discharge scan accumulates over the live snapshot. They bound the
/// work an apply stages and let apply re-count to detect drift.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DischargeCounts {
    /// Records visited across every scanned root.
    pub scanned_records: usize,
    /// Records missing a member an obligation requires.
    pub records_lacking_member: usize,
    /// Distinct unique-index key tuples that more than one record claims.
    pub index_collisions: usize,
    /// Records a defaulting obligation must backfill.
    pub records_to_backfill: usize,
}

/// A constant fill value for a defaulting obligation, already encoded in its
/// canonical saved form. Apply writes `encoded` directly at the member cell;
/// `scalar_type` is the leaf type it decodes and validates against. An evolve
/// default must be a constant the checker evaluates at discharge time, so the fill
/// value is self-contained and apply needs no source expression to backfill with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultValue {
    pub scalar_type: ScalarType,
    pub encoded: Vec<u8>,
}

/// The discharge classification of one obligation. The user vocabulary is
/// rename/default/prove/transform/retire/rebuild/repair; each verdict names the
/// role the snapshot plays for that obligation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// No durable work: an optional member, or a change that does not touch stored
    /// data. (no-op)
    NoOp,
    /// A rename moved a catalog binding with no record data to move. (rename)
    CatalogOnly,
    /// A source-dropped entry nothing depends on; its data lingers and the entry is
    /// deprecated rather than retired. (retire, soft)
    Deprecated,
    /// Every record already carries the member, so its presence claim is proven
    /// against the snapshot. (prove)
    DataProof,
    /// A newly-required member backfilled from a constant evolve default. (default)
    Default { value: DefaultValue },
    /// A new index whose entries apply rebuilds from the records it covers.
    /// (rebuild)
    DerivedRebuild,
    /// A change of stored meaning that is not applyable without a checked
    /// transform, so the obligation blocks activation. (transform)
    TypedTransformRequired,
    /// A retire over populated data; apply needs a scoped approval that names this
    /// catalog id and matches the recorded populated count. (retire, hard)
    DestructiveDecisionRequired { populated: usize },
    /// The snapshot cannot satisfy the obligation; activation fails closed.
    /// (repair)
    RepairRequired { reason: RepairReason },
}

/// Why an obligation cannot be discharged against the current snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairReason {
    /// A newly-required member is absent from records that carry no default.
    MissingRequiredMember,
    /// A unique index covers records whose full key tuples collide.
    UniqueIndexCollision,
    /// A source-dropped member is still read by an active index, so it cannot be
    /// silently deprecated: the change needs an explicit retire intent that also
    /// removes or rebinds the index. The verdict carries the index's catalog
    /// identity, not its developer-facing name; the name lives in the fail-closed
    /// diagnostic the discharge emits.
    RetireRequired { index: CatalogId },
}

impl Verdict {
    /// Whether this outcome leaves the program activatable with no further developer
    /// decision: nothing to do, identity-only, a proven claim, a recorded default,
    /// or a derived rebuild. A transform, a destructive approval, or a repair blocks
    /// activation until a decision or fix lands.
    pub fn is_activatable(&self) -> bool {
        matches!(
            self,
            Verdict::NoOp
                | Verdict::CatalogOnly
                | Verdict::Deprecated
                | Verdict::DataProof
                | Verdict::Default { .. }
                | Verdict::DerivedRebuild
        )
    }
}

/// One obligation's discharge classification as it crosses into apply: the catalog
/// id the change touches and the typed verdict apply re-verifies. No human prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObligationVerdict {
    pub catalog_id: CatalogId,
    pub verdict: Verdict,
}

/// The composed evolution witness. It is the only thing that crosses the
/// check->run boundary into apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvolutionWitness {
    /// A digest of the analyzed durable surface and the evolve decision surface, in
    /// the existing `fnv1a64:<hex>` form. Apply re-derives it from the source it
    /// activates and aborts on drift.
    pub source_digest: String,
    /// The accepted catalog the snapshot was computed against.
    pub accepted_catalog: CatalogFingerprint,
    /// The catalog proposal to activate, or `None` when source proposed no change.
    pub proposal_catalog: Option<CatalogFingerprint>,
    /// The store's engine-profile digest, or `None` for an empty store.
    pub engine_profile_digest: Option<EngineProfileDigest>,
    /// The store's layout epoch, or `None` for an empty store.
    pub layout_epoch: Option<u64>,
    /// The latest store commit id, or `None` if the store has never committed.
    pub store_commit_id: Option<u64>,
    /// The catalog ids the change touches, sorted and deduplicated.
    pub affected_catalog_ids: Vec<CatalogId>,
    /// The per-obligation discharge verdicts.
    pub verdicts: Vec<ObligationVerdict>,
    /// The counts the discharge scan accumulated.
    pub counts: DischargeCounts,
}

impl EvolutionWitness {
    /// The program is activatable exactly when every obligation is discharged
    /// without a pending developer decision or a fail-closed repair.
    pub fn is_activatable(&self) -> bool {
        self.verdicts
            .iter()
            .all(|outcome| outcome.verdict.is_activatable())
    }
}
