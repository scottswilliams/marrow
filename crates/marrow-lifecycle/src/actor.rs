//! Persistent attachment and binding-only code updates.
//!
//! Under the retained directory owner, admission checks the accepted ceiling,
//! compatible binding facts and complete image/projection/Head correspondence before
//! opening an engine. The resulting numbered projection carries the Head's addresses.
//! An exact active image leaves Head and Envelope unchanged. A code-only rebind
//! publishes Pending, then Head, then Active with their durability barriers before
//! returning a receipt. Changed contracts are typed refusals; they never construct
//! a semantic store handle. NativeAttachment keeps the admitted image and owner together.

use std::path::Path;

use marrow_codes::Code;
use marrow_image::{CeilingDescriptor, ExportDemand};
use marrow_kernel::durable::{NumberedProjection, StoreProjection};
use marrow_verify::VerifiedImage;

use crate::attachment::{Attachment, NativeAttachment, PreparedImage};
use crate::audit::{self, AuditError, Names, StoreAudit};
use crate::authority::{self, DemandExceedsCeiling};
use crate::head::{ActiveBinding, LogicalHead};
use crate::image::{
    HeadMapPinMismatch, PinDisagreement, ProjectedNodes, active_binding, derive_projection_nodes,
};
use crate::provision::{LockedStore, OpenBinding, OpenError};
use crate::store_dir;

/// Why the admission gate declined before any engine call.
///
/// One enum for both strictnesses: the ceiling and pin facts are the same
/// whichever gate ran, and the binding arms differ only in which comparison
/// reached them. Callers render it into their own reported error; none re-wraps
/// it in a parallel refusal type.
pub(crate) enum AdmissionRefusal {
    /// The image's demand exceeds the accepted ceiling — a typed authority refusal.
    Exceeds(DemandExceedsCeiling),
    /// The persisted accepted-ceiling payload did not decode — store corruption.
    CeilingCorrupt,
    /// The persisted head-map pin disagrees with the numbering this toolchain derives —
    /// fail-closed, the store is never attached under a disagreeing numbering.
    Pin(HeadMapPinMismatch),
    /// The head binds a different image with different binding facts.
    ContractChanged(ContractChanged),
    /// Exact admission only: the head binds a different image whose binding facts are
    /// equal, so the presented image is a code-only edit the store has not been rebound to.
    NotActive,
    /// Exact admission only: the head names this image's identity but records binding facts
    /// the image does not have — inconsistent binding metadata, recovery-shaped.
    InconsistentBinding,
}

impl AdmissionRefusal {
    /// The open error a corrupt persisted ceiling payload reports.
    pub(crate) fn ceiling_corrupt() -> OpenError {
        OpenError::Corruption {
            message: "the persisted accepted authority ceiling did not decode".to_string(),
        }
    }
}

/// How strictly the persisted head must already bind the presented image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindingStrictness {
    /// The durable contract must match; a code-only difference is admitted and the caller
    /// rebinds. The ceiling is judged first, so an over-demanding image is refused on
    /// authority even when its contract also changed.
    Compatible,
    /// The head must already bind exactly this image. The binding is judged first, so a
    /// stale or foreign image is named as such before its demand is measured.
    Exact,
}

/// One image's active binding, occurrence correspondence and owned projection.
/// Successful admission consumes them into the exact accepted numbered layout.
pub(crate) struct ImageAdmission<'a> {
    image: &'a VerifiedImage,
    incoming: ActiveBinding,
    nodes: Result<ProjectedNodes, HeadMapPinMismatch>,
    projection: StoreProjection,
}

impl<'a> ImageAdmission<'a> {
    pub(crate) fn derive(image: &'a VerifiedImage, projection: StoreProjection) -> Self {
        Self {
            image,
            incoming: active_binding(image),
            nodes: derive_projection_nodes(image, &projection),
            projection,
        }
    }

    /// The presented image's active binding.
    pub(crate) fn incoming(&self) -> &ActiveBinding {
        &self.incoming
    }

    pub(crate) fn audit_names(&self) -> Names {
        Names::new(&self.projection)
    }

    /// Admit the presented image against `head` at `strictness` and mint the accepted
    /// numbered layout. An incompatible graph has no mapping for this handle and never
    /// opens an engine.
    pub(crate) fn admit(
        self,
        head: &LogicalHead,
        strictness: BindingStrictness,
    ) -> Result<NumberedProjection, AdmissionRefusal> {
        match strictness {
            BindingStrictness::Compatible => {
                self.admit_ceiling(head)?;
                self.require_compatible_binding(head)?;
            }
            BindingStrictness::Exact => {
                self.require_exact_binding(head)?;
                self.admit_ceiling(head)?;
            }
        }
        self.numbered(head).map_err(AdmissionRefusal::Pin)
    }

    /// Admit exactly, reusing the demand already derived from this admission's image.
    /// The caller must supply that same image's demand, not a subset or another image's.
    pub(crate) fn admit_exact_with_demand(
        self,
        head: &LogicalHead,
        demand: &ExportDemand,
    ) -> Result<NumberedProjection, AdmissionRefusal> {
        self.require_exact_binding(head)?;
        self.admit_ceiling_demand(head, demand)?;
        self.numbered(head).map_err(AdmissionRefusal::Pin)
    }

    /// The durable contract must be unchanged; a code-only difference is admitted.
    fn require_compatible_binding(&self, head: &LogicalHead) -> Result<(), AdmissionRefusal> {
        if self.incoming.facts_equal(&head.binding) {
            return Ok(());
        }
        Err(AdmissionRefusal::ContractChanged(ContractChanged {
            changed: classify_delta(&head.binding, &self.incoming),
        }))
    }

    /// The head must already name exactly this image with exactly its facts.
    fn require_exact_binding(&self, head: &LogicalHead) -> Result<(), AdmissionRefusal> {
        let stored = &head.binding;
        if self.incoming == *stored {
            return Ok(());
        }
        Err(if self.incoming.image_id == stored.image_id {
            AdmissionRefusal::InconsistentBinding
        } else if stored.facts_equal(&self.incoming) {
            AdmissionRefusal::NotActive
        } else {
            AdmissionRefusal::ContractChanged(ContractChanged {
                changed: classify_delta(stored, &self.incoming),
            })
        })
    }

    /// Reconstruct the accepted ceiling from the persisted head and intersect it with the
    /// presented image's whole-program demand (see `authority::admit_demand`). A ceiling payload
    /// that does not decode is store corruption, not a demand refusal.
    fn admit_ceiling(&self, head: &LogicalHead) -> Result<(), AdmissionRefusal> {
        self.admit_ceiling_demand(head, &self.image.demand_union())
    }

    fn admit_ceiling_demand(
        &self,
        head: &LogicalHead,
        demand: &ExportDemand,
    ) -> Result<(), AdmissionRefusal> {
        let accepted = CeilingDescriptor::from_payload(&head.accepted_ceiling)
            .map_err(|_| AdmissionRefusal::CeilingCorrupt)?;
        authority::admit_demand(self.image, demand, &accepted).map_err(AdmissionRefusal::Exceeds)
    }

    /// Resolve accepted physical addresses only after semantic correspondence succeeds.
    fn numbered(self, head: &LogicalHead) -> Result<NumberedProjection, HeadMapPinMismatch> {
        let numbers = self.nodes?.accepted_numbers(&head.head_map)?;
        NumberedProjection::accepted(self.projection, &numbers, head.head_map.next_number())
            .map_err(|error| HeadMapPinMismatch {
                disagreement: PinDisagreement::Numbering(error),
            })
    }
}

/// The result of a successful attach: the admitted image paired with the open store.
pub enum AttachOutcome {
    /// The presented image is already the active binding: the head and envelope are
    /// byte-unchanged (taking the lock rewrote its owner marker first, as on every attach).
    /// The store is open and ready.
    AlreadyActive(NativeAttachment),
    /// The image was a binding-only code update. Pending, the new head and final Active
    /// each passed their directory barrier. The receipt reports the resulting binding.
    Rebound {
        attachment: NativeAttachment,
        receipt: RebindReceipt,
    },
}

/// What a binding-only rebind reports: the store instance and the newly active image
/// identity, returned only after Pending, head and final Active directory barriers.
/// Reading one from an
/// [`AttachOutcome::Rebound`] therefore means "the active code was updated, the durable
/// contract unchanged" — that is the actor's guarantee about the value it returned, not a
/// property of the value itself. The fields are public and `StoreInstanceId::from_bytes` is
/// public, so an equal value is constructible without any rebind: this is a record, not an
/// unforgeable token, and nothing may authorize on having one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RebindReceipt {
    pub instance: crate::instance::StoreInstanceId,
    pub new_image_id: [u8; 32],
}

/// Which binding fact differs — the category a contract-changed refusal names. Authority is
/// not a binding fact: a demand change that exceeds the accepted ceiling is the distinct,
/// more actionable [`DemandExceedsCeiling`] refusal, and a demand change within it is
/// admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangedFact {
    /// The durable contract — the durable graph over ledger ids — changed (an evolution).
    DurableContract,
    /// The exported interface changed — today the export *set* (the declaration-path
    /// fingerprint the head pins), so an added, removed, renamed, or relocated export is
    /// caught here while a resignatured export is not; the signature-sensitive verified
    /// interface binding that closes that gap is future work.
    Interface,
}

impl ChangedFact {
    fn describe(self) -> &'static str {
        match self {
            ChangedFact::DurableContract => "the durable contract",
            ChangedFact::Interface => "the exported interface",
        }
    }
}

/// A binding-fact delta refused by the attempted operation. This does not establish
/// integrity or service readiness of the store's current binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContractChanged {
    pub changed: ChangedFact,
}

impl ContractChanged {
    /// The stable dotted code — `store.contract_changed`, a typed lifecycle refusal, never
    /// `store.corruption`.
    pub fn code(&self) -> Code {
        Code::StoreContractChanged
    }
}

impl std::fmt::Display for ContractChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the supplied image differs in {} from the binding required by this operation; \
             the current binding was not changed",
            self.changed.describe(),
        )
    }
}

/// Why an attach failed.
#[derive(Debug)]
pub enum LifecycleError {
    /// The image's durable shape is not executable by the store kernel (a storeless image or
    /// a parked shape), so no store can be opened for it. Decided before the store is touched.
    NotExecutable,
    /// The store could not be opened (not provisioned, incomplete, held by another owner, or
    /// corrupt).
    Open(OpenError),
    /// The presented image's verified demand exceeds the store's accepted authority ceiling
    /// — a typed refusal naming the exceeding export, effect, and place, with zero engine
    /// calls, never corruption. The owner must consciously expand the accepted ceiling.
    DemandExceedsCeiling(DemandExceedsCeiling),
    /// The image is not a binding-only code update — a typed refusal pointing at `marrow
    /// apply`, never corruption.
    ContractChanged(ContractChanged),
    /// The store's persisted head-map pin (the ledger-id ↔ cell-number bijection)
    /// disagrees with the (ledger id → cell number) binding this toolchain would serve the
    /// store under. Fail-closed and recovery-shaped: serving the store would readdress
    /// durable cells, so the attach refuses with zero engine calls; Head, envelope,
    /// engine data and the owner marker are unchanged.
    HeadMapPin(HeadMapPinMismatch),
    /// The read-only logical audit or prepublication metadata verification failed.
    /// Metadata verification follows writable preparation and may observe its bookkeeping.
    Audit(AuditError),
    /// Read-only admission found inconsistent stored data.
    Invalid(Box<StoreAudit>),
    /// Rewriting the envelope or head during a rebind failed.
    Metadata(store_dir::AdmissionError),
    /// Earlier rebind barriers passed, but final activation was not confirmed.
    ActivationUncertain {
        instance: crate::StoreInstanceId,
        source: AuditError,
    },
}

impl From<AdmissionRefusal> for LifecycleError {
    fn from(refusal: AdmissionRefusal) -> Self {
        match refusal {
            AdmissionRefusal::Exceeds(refusal) => Self::DemandExceedsCeiling(refusal),
            AdmissionRefusal::CeilingCorrupt => Self::Open(AdmissionRefusal::ceiling_corrupt()),
            AdmissionRefusal::Pin(refusal) => Self::HeadMapPin(refusal),
            AdmissionRefusal::ContractChanged(refusal) => Self::ContractChanged(refusal),
            AdmissionRefusal::NotActive | AdmissionRefusal::InconsistentBinding => {
                Self::Open(OpenError::Corruption {
                    message: "the store's head does not bind the presented image".to_string(),
                })
            }
        }
    }
}

impl LifecycleError {
    /// The stable dotted code a tool reports.
    pub fn code(&self) -> Code {
        match self {
            LifecycleError::NotExecutable => Code::CliDurableUnsupported,
            LifecycleError::Open(error) => error.code(),
            LifecycleError::DemandExceedsCeiling(refusal) => refusal.code(),
            LifecycleError::ContractChanged(refusal) => refusal.code(),
            LifecycleError::HeadMapPin(refusal) => refusal.code(),
            LifecycleError::Audit(error) => error.code(),
            LifecycleError::Invalid(_) => Code::StoreCorruption,
            LifecycleError::Metadata(error) => error.code(),
            LifecycleError::ActivationUncertain { .. } => Code::StoreActivationUncertain,
        }
    }
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LifecycleError::NotExecutable => write!(
                f,
                "the program's durable shape is not yet executable by the store"
            ),
            LifecycleError::Open(error) => write!(f, "{error}"),
            LifecycleError::DemandExceedsCeiling(refusal) => write!(f, "{refusal}"),
            LifecycleError::ContractChanged(refusal) => write!(f, "{refusal}"),
            LifecycleError::HeadMapPin(refusal) => write!(f, "{refusal}"),
            LifecycleError::Audit(error) => write!(f, "{error}"),
            LifecycleError::Invalid(report) => write!(
                f,
                "binding admission found {} stored-data inconsistencies",
                report.summary.findings
            ),
            LifecycleError::Metadata(error) => {
                write!(f, "rebind metadata update failed: {error}")
            }
            LifecycleError::ActivationUncertain { instance, source } => write!(
                f,
                "store {} rebind activation is unconfirmed: {source}",
                instance.to_hex()
            ),
        }
    }
}

impl std::error::Error for LifecycleError {}

/// Attach under the store's directory owner and accepted numbered projection.
/// Semantic refusal precedes marker mutation and engine opening. An exact binding
/// opens ordinary service. A code-only transition completes read-only logical
/// admission and prepares writable service under continuous cooperative ownership.
/// Unchanged admitted bytes avoid full repair; external same-inode mutation is
/// not detected. An inherited physical-audit failure stops publication. The actor verifies the old
/// metadata, then publishes Pending, Head and Active before final verification and
/// a receipt.
pub fn attach(dir: &Path, prepared: PreparedImage) -> Result<AttachOutcome, LifecycleError> {
    let (image, projection) = prepared.into_parts();
    let Some(projection) = projection else {
        return Err(LifecycleError::NotExecutable);
    };

    // The admission facts are pure over (image, projection); derived here, before the store
    // is touched, so the gate below needs no borrow of the projection the open consumes. The
    // gate runs after the single-owner lock and before any engine call, so a refusal makes
    // zero engine calls.
    let admission = ImageAdmission::derive(&image, projection);
    let incoming = *admission.incoming();
    let binding = LockedStore::acquire(dir)
        .map_err(LifecycleError::Open)?
        .open_compatible(admission)?;
    let (opened, names) = match binding {
        OpenBinding::Active(opened) => {
            return Ok(AttachOutcome::AlreadyActive(Attachment::new(image, opened)));
        }
        OpenBinding::Rebind { opened, names } => (opened, names),
    };

    let report =
        audit::inspect(&opened, &names, image.image_id()).map_err(LifecycleError::Audit)?;
    if !report.is_clean() {
        return Err(LifecycleError::Invalid(Box::new(report)));
    }
    #[cfg(test)]
    binding_fault::check(&opened.directory, dir, binding_fault::Point::Admitted)
        .map_err(LifecycleError::Open)?;
    let mut opened = opened.into_service().map_err(LifecycleError::Open)?;
    #[cfg(test)]
    binding_fault::check(&opened.directory, dir, binding_fault::Point::Prepared)
        .map_err(LifecycleError::Open)?;
    let old_record = crate::envelope::EnvelopeRecord {
        metadata: opened.envelope.clone(),
        state: crate::envelope::EnvelopeState::Active,
    };
    audit::verify_published(&opened.directory, dir, &old_record, opened.head_digest)
        .map_err(LifecycleError::Audit)?;

    // Binding-only rebind: the durable contract, interface, and ceiling are unchanged and
    // only the image code differs. Persist Pending before the head change, preserving the
    // head map and reserved slots, and return service only after durable Active completion.
    let new_envelope = crate::envelope::StoreEnvelope {
        writer_toolchain: current_toolchain(),
        ..opened.envelope.clone()
    };
    let new_head = LogicalHead {
        binding: incoming,
        ..opened.head.clone()
    };
    let new_digest = rewrite_atomically(
        &opened.directory,
        dir,
        &new_envelope,
        &new_head,
        opened.head_digest,
    )?;

    let receipt = RebindReceipt {
        instance: new_envelope.instance,
        new_image_id: incoming.image_id,
    };
    opened.envelope = new_envelope;
    opened.head = new_head;
    opened.head_digest = new_digest;
    Ok(AttachOutcome::Rebound {
        attachment: Attachment::new(image, opened),
        receipt,
    })
}

/// The binding fact that differs between the store's active binding and the incoming image,
/// checked in a fixed order (durable contract, then interface). At least one differs because
/// the caller has established `!facts_equal`.
fn classify_delta(stored: &ActiveBinding, incoming: &ActiveBinding) -> ChangedFact {
    if stored.durable_contract != incoming.durable_contract {
        ChangedFact::DurableContract
    } else {
        ChangedFact::Interface
    }
}

/// The exact released toolchain version performing this write, recorded in the envelope's
/// writer tuple.
pub(crate) fn current_toolchain() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Persist Pending and its version discriminator before changing the head. Active may
/// replace it only after the new head's directory barrier. No receipt precedes the final sync.
pub(crate) fn rewrite_atomically(
    dir: &store_dir::AdmittedStoreDir,
    location: &Path,
    envelope: &crate::envelope::StoreEnvelope,
    head: &LogicalHead,
    old: marrow_image::StoreHeadDigest,
) -> Result<marrow_image::StoreHeadDigest, LifecycleError> {
    use crate::envelope::{EnvelopeRecord, EnvelopeState};
    use store_dir::{AdmissionError, Artifact, StoreEntry};
    let (head_bytes, new) = head.encode_with_digest();
    let mut record = EnvelopeRecord {
        metadata: envelope.clone(),
        state: EnvelopeState::Rebind { old, new },
    };
    let persist_pending_head = || -> Result<(), AdmissionError> {
        dir.replace(
            Artifact::Envelope,
            &record
                .encode()
                .map_err(|error| AdmissionError::format(StoreEntry::Envelope, error))?,
        )?;
        #[cfg(test)]
        store_dir::barrier_fault::check(dir, store_dir::barrier_fault::Point::RebindPending)?;
        dir.sync()?;
        dir.replace(Artifact::Head, &head_bytes)?;
        #[cfg(test)]
        store_dir::barrier_fault::check(dir, store_dir::barrier_fault::Point::RebindHead)?;
        dir.sync()
    };
    persist_pending_head().map_err(LifecycleError::Metadata)?;
    record.state = EnvelopeState::Active;
    let activate = || -> Result<(), AuditError> {
        let metadata_error = |error| AuditError::Open(OpenError::Admission(error));
        dir.replace(
            Artifact::Envelope,
            &record.encode().map_err(|error| {
                metadata_error(AdmissionError::format(StoreEntry::Envelope, error))
            })?,
        )
        .map_err(metadata_error)?;
        #[cfg(test)]
        store_dir::barrier_fault::check(dir, store_dir::barrier_fault::Point::RebindActive)
            .map_err(metadata_error)?;
        dir.sync().map_err(metadata_error)?;
        #[cfg(test)]
        binding_fault::check(dir, location, binding_fault::Point::Activated)
            .map_err(AuditError::Open)?;
        audit::verify_published(dir, location, &record, new)
    };
    activate().map_err(|source| LifecycleError::ActivationUncertain {
        instance: envelope.instance,
        source,
    })?;
    Ok(new)
}

#[cfg(test)]
pub(crate) mod binding_fault {
    use super::*;
    use std::path::PathBuf;
    use store_dir::{AdmittedStoreDir, Artifact};

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Point {
        Admitted,
        Prepared,
        Activated,
    }

    pub(crate) enum Mutation {
        Engine(PathBuf),
        Metadata(Artifact, Vec<u8>),
        Directory(PathBuf),
    }

    struct Armed {
        identity: marrow_fs_journal::FsIdentity,
        point: Point,
        mutation: Mutation,
    }
    thread_local! {
        static ARMED: std::cell::RefCell<Option<Armed>> = const { std::cell::RefCell::new(None) };
    }

    pub(crate) fn with_mutation<T>(
        location: &Path,
        point: Point,
        mutation: Mutation,
        action: impl FnOnce() -> T,
    ) -> T {
        struct Restore(Option<Armed>);
        impl Drop for Restore {
            fn drop(&mut self) {
                ARMED.with(|slot| *slot.borrow_mut() = self.0.take());
            }
        }
        let location = std::fs::canonicalize(location).expect("test store");
        let identity = AdmittedStoreDir::admit(&location)
            .expect("test directory identity")
            .identity();
        let _restore = Restore(ARMED.with(|slot| {
            slot.replace(Some(Armed {
                identity,
                point,
                mutation,
            }))
        }));
        let result = action();
        assert!(
            ARMED.with(|slot| slot.borrow().is_none()),
            "binding checkpoint was not reached"
        );
        result
    }

    pub(super) fn check(
        dir: &AdmittedStoreDir,
        location: &Path,
        point: Point,
    ) -> Result<(), OpenError> {
        let mutation = ARMED.with(|slot| {
            let mut armed = slot.borrow_mut();
            if armed
                .as_ref()
                .is_some_and(|armed| armed.identity == dir.identity() && armed.point == point)
            {
                armed.take().map(|armed| armed.mutation)
            } else {
                None
            }
        });
        match mutation {
            Some(Mutation::Engine(displaced)) => {
                let engine = location.join(store_dir::ENGINE_FILE);
                std::fs::rename(&engine, &displaced).map_err(OpenError::Io)?;
                std::fs::copy(&displaced, &engine).map_err(OpenError::Io)?;
            }
            Some(Mutation::Metadata(artifact, bytes)) => {
                dir.replace(artifact, &bytes)
                    .map_err(OpenError::Admission)?;
                dir.sync().map_err(OpenError::Admission)?;
            }
            Some(Mutation::Directory(displaced)) => {
                std::fs::rename(location, displaced).map_err(OpenError::Io)?;
            }
            None => {}
        }
        Ok(())
    }
}
