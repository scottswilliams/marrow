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
    /// Records an applyable transform recomputes. A transform rewrites its target cell
    /// for every record under the place, so the count is the record total of each
    /// applyable transform's place; apply re-counts and refuses on drift.
    pub records_to_transform: usize,
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
    /// No durable work: an optional member, a dependency-free dropped sparse field
    /// whose data harmlessly lingers, or a change that does not touch stored data.
    /// (no-op)
    NoOp,
    /// A rename moved a catalog binding with no record data to move. (rename)
    CatalogOnly,
    /// A source-dropped store index. Its catalog binding is gone, so apply deletes the
    /// derived index-cell subtree under its catalog id; no source data moves. (rebuild,
    /// inverse)
    IndexDropped,
    /// Every record already carries the member, so its presence claim is proven
    /// against the snapshot. (prove)
    DataProof,
    /// A newly-required member backfilled from a constant evolve default. (default)
    Default { value: DefaultValue },
    /// A new index whose entries apply rebuilds from the records it covers.
    /// (rebuild)
    DerivedRebuild,
    /// A checked `evolve transform` recomputes this member per record from the read
    /// members it names. Apply binds each record's read-member values as `old`,
    /// evaluates the pure transform body, and writes the result. The reads are the
    /// catalog ids of the members the body reads via `old.<member>`; their old bytes
    /// must decode under their current type, proven by a sibling decodability
    /// obligation, so reading them is sound. (transform)
    Transform { reads: Vec<CatalogId> },
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
    /// A newly-required member declares an `evolve default`, but the value is not a
    /// constant the checker can carry as a typed fill. The developer named a fill, so this
    /// is distinct from `MissingRequiredMember`: the obligation is not "no default" but "a
    /// default that cannot be encoded", and the typed cause names which way it failed.
    DefaultRejected { reason: RejectedDefault },
    /// A stored cell carries bytes that are present but not valid under the member's
    /// current type: they fail to decode, or they name an enum member the current enum
    /// no longer offers as a value (one removed, made `category`, or given children). The
    /// value is not missing — it is there and wrong — so it cannot be backfilled away; the
    /// record must be repaired to a valid value before activation.
    InvalidStoredValue,
    /// A unique index covers records whose full key tuples collide.
    UniqueIndexCollision,
    /// A unique index cannot be probed for collisions from the current snapshot: a key
    /// column resolves to neither an identity-key position nor a top-level plain field, so
    /// the scan cannot derive the prospective key tuples to check uniqueness. Rather than
    /// rebuild an unchecked unique index, the change fails closed until the index key shape
    /// is one the collision scan can probe.
    UniqueIndexUnprobeable,
    /// A source-dropped member is still read by an active index, so it cannot be
    /// silently retired: the change needs an explicit retire intent that also
    /// removes or rebinds the index. The verdict carries the index's catalog
    /// identity, not its developer-facing name; the name lives in the fail-closed
    /// diagnostic the discharge emits.
    RetireRequired { index: CatalogId },
    /// A source-dropped member or store still holds stored data, but source declares no
    /// `evolve retire` intent for it. Bare deletion would orphan that data silently, so the
    /// change fails closed until the developer states the destructive intent with `evolve
    /// retire`, which then routes the drop through the approved-retire path. An unpopulated
    /// member or store has nothing to orphan and stays a free no-op; this fences only the
    /// populated case.
    PopulatedDropRequiresRetire,
    /// A retire targets a member nested under an unkeyed group or a keyed layer. A
    /// top-level retire drops a cell directly under the record node, but a nested
    /// retire must descend the owning group or page each keyed entry to find the
    /// cells to drop, which the apply path does not yet do. Until it does, a nested
    /// retire fails closed rather than counting zero populated cells and silently
    /// dropping nothing.
    NestedRetireUnsupported,
    /// A member an `evolve transform` reads via `old.<member>` has stored bytes that
    /// do not decode under its current type. The transform's soundness rests on every
    /// read member's old bytes decoding as the current type (unchanged or compatibly
    /// widened); a record whose bytes were written under an incompatible type cannot
    /// be read, so the transform fails closed until that data is repaired.
    UndecodableTransformInput,
    /// A populated member's declared leaf type changed from the type its durable bytes
    /// were accepted as. The bytes were written under the old type, so reading them as
    /// the new type would reinterpret their meaning — silently for an overlapping codec
    /// (an `int` stored as `1` reads as `bool` `true`), or as a decode failure for a
    /// disjoint one. v0.1 fails every leaf type change on populated data closed here. An
    /// in-place transform cannot resolve it, because a transform may not read the member
    /// it replaces; the supported path is to add a new member of the new type, populate
    /// it with an `evolve transform` computed from this one, then retire this member.
    TypeChangeRequiresTransform,
    /// A store's identity-key shape — its key arity or any key type — changed from the shape
    /// its durable records were keyed under. Every record is addressed by the old key bytes,
    /// which sit in the saved path itself, so a record keyed by an `int` cannot be found under
    /// a `string` key and a single-key record cannot be found under a composite key. v0.1 has
    /// no graceful store-key migration: re-keying would orphan the existing data, so the change
    /// fails closed here rather than activating against records the new key shape cannot reach.
    StoreKeyShapeChange,
    /// A nested keyed-layer member's key shape — its key arity or any key type — changed, or a
    /// plain group was reshaped into a keyed layer (or the reverse). Each per-entry value is
    /// addressed by its entry key bytes in the saved path, one level below the store key, so a
    /// re-keyed or reshaped layer addresses none of the existing entries. v0.1 has no graceful
    /// keyed-layer migration, so the change fails closed rather than activating over per-entry
    /// data the new layer shape cannot reach. This is the store-key-shape rule one level down.
    KeyedLayerKeyShapeChange,
    /// A durable member's structural signature changed in a way no targeted classifier handled
    /// and existing data is present. The signature records the member's kind, its key shape if
    /// keyed, and its leaf token if a leaf, all identity-aware; a difference the leaf-type,
    /// reshape, rename, default, transform, and retire classifiers all left unclaimed is a
    /// transition v0.1 does not know how to apply to stored data. This is the default-deny
    /// backstop that keeps the fail-closed invariant total: any unhandled structural divergence
    /// over populated data fails closed here rather than silently activating.
    StructuralDivergence,
}

/// Why a declared `evolve default` could not be carried as a typed constant fill. The
/// developer named a default, so the obligation fails closed on the specific way the value
/// was rejected rather than on the absence of a default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectedDefault {
    /// The value is not a constant the checker evaluates at discharge time; a per-record
    /// fill is a transform, not a default.
    NotConstant,
    /// The value is a constant of the wrong type for the member's leaf.
    TypeMismatch,
    /// The value is a constant of the right type but out of the leaf's encodable range.
    NotEncodable,
}

impl RejectedDefault {
    /// The render-only prose a rejected default reports. Logic branches on the variant; this
    /// is the one place its developer-facing wording lives, so the discharge diagnostic and
    /// any boundary renderer never spell the cause twice.
    pub fn message(&self) -> &'static str {
        match self {
            RejectedDefault::NotConstant => {
                "evolve default must be a constant value; use a transform for computed values"
            }
            RejectedDefault::TypeMismatch => "evolve default value type does not match the member",
            RejectedDefault::NotEncodable => "evolve default value is out of range for the member",
        }
    }
}

impl Verdict {
    /// Whether this outcome leaves the program activatable with no further developer
    /// decision: nothing to do, identity-only, a proven claim, a recorded default, a
    /// derived rebuild, or a checked transform whose read members all decode. A
    /// destructive approval or a repair blocks activation until a decision or fix
    /// lands.
    pub fn is_activatable(&self) -> bool {
        // The activatable variants are exactly those whose durable work apply can derive
        // and perform on its own. `Default` carries a checked constant to backfill and
        // `Transform` carries a checked pure recompute whose read members all decode, so
        // both run unattended. The blocking variants need a human input apply cannot
        // supply: `DestructiveDecisionRequired` needs a scoped approval to drop data, and
        // `RepairRequired` needs the snapshot fixed before the obligation can hold at all.
        matches!(
            self,
            Verdict::NoOp
                | Verdict::CatalogOnly
                | Verdict::IndexDropped
                | Verdict::DataProof
                | Verdict::Default { .. }
                | Verdict::DerivedRebuild
                | Verdict::Transform { .. }
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
    /// A digest of the analyzed durable shape, in the `sha256:<hex>` form.
    /// This is the digest apply stamps and the activation-window fence enforces, so it
    /// excludes the transient evolve block; a consumed block is therefore deletable
    /// without reading as schema drift.
    pub source_digest: String,
    /// A digest of the durable shape *and* the evolve decision surface. Apply re-derives
    /// it from the source it activates and aborts on drift, so a transform-body or
    /// evolve-default edit between preview and apply fails closed even though the shape
    /// digest the store stamps does not move.
    pub evolution_digest: String,
    /// The accepted catalog the snapshot was computed against.
    pub accepted_catalog: CatalogFingerprint,
    /// The catalog proposal to activate, or `None` when source proposed no change.
    pub proposal_catalog: Option<CatalogFingerprint>,
    /// The accepted catalog snapshot the store held when preview discharged the
    /// witness. Apply permits a file-ahead activation only when this exact older
    /// snapshot is still present, so a stale local store can catch up to the committed
    /// file artifact without accepting concurrent catalog drift.
    pub store_catalog: Option<CatalogFingerprint>,
    /// The store's stamped shape digest at preview time, or `None` for an unstamped
    /// store. Apply fences the store against this so an evolution that changes shape
    /// verifies the store still holds the pre-apply shape before stamping the new one,
    /// rather than fencing the store against the very shape apply is moving it to.
    pub store_source_digest: Option<String>,
    /// The store's engine-profile digest, or `None` for an empty store.
    pub engine_profile_digest: Option<EngineProfileDigest>,
    /// The store's layout epoch, or `None` for an empty store.
    pub layout_epoch: Option<u64>,
    /// The latest store commit id, or `None` if the store has never committed.
    pub store_commit_id: Option<u64>,
    /// The data-root catalog ids the change touches: resources, stores, members,
    /// enums. Sorted and deduplicated. Apply stamps these as the changed roots so a
    /// dropped index id is never mistaken for a root.
    pub changed_root_catalog_ids: Vec<CatalogId>,
    /// The store-index catalog ids the change touches, sorted and deduplicated. The
    /// discharge tags each id at classify time by its catalog entry kind, so apply
    /// never re-derives the index set from current source.
    pub changed_index_catalog_ids: Vec<CatalogId>,
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
