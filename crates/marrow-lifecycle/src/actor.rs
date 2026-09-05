//! The one-shot lifecycle actor: the only path that pairs a prepared image with a persistent
//! store, the only one that can change which verified program image a store is bound to, and
//! the only one that compares the store's head-map pin.
//!
//! An attach consumes a [`PreparedImage`], takes the store's single-owner lock, rereads the
//! persisted head from disk (never a cached copy), and classifies the image against the
//! store's active binding:
//!
//! - **Already active** — the image is byte-identical to the active binding; the store opens
//!   with the head and envelope byte-unchanged. Taking the lock has already rewritten its
//!   owner marker, as it does on every attach before anything is classified.
//! - **Demand exceeds ceiling** — the presented image demands durable authority the store's
//!   accepted ceiling (its separately owned standing maximum, recorded at provision) does not
//!   admit. Checked first, after the lock and before any engine call, so a broadened effect
//!   is refused with zero engine calls, naming the exceeding export, effect, and place in
//!   source vocabulary (`authority::admit`). Not corruption: the store is intact.
//! - **Head-map pin disagreement** — the persisted ledger-id ↔ cell-number bijection
//!   (FR01 §3) disagrees with the (ledger id → cell number) binding this toolchain would
//!   actually serve the store under: the kernel's numbering of the exact projection this
//!   open installs, paired to the image's durable identities by source name and node kind
//!   (`crate::image::derive_head_map_pin`). Checked with the admission gate, before any
//!   engine call: a store is never attached under a numbering that disagrees with its
//!   persisted pin, because serving it would readdress durable cells. Fail-closed and
//!   recovery-shaped; the head, envelope, and engine data are unchanged (acquisition has
//!   already rewritten the lock's owner marker, so the next successful open audits).
//! - **Binding-only rebind** — the durable contract and interface are unchanged and only the
//!   image's code (its byte identity) differs. The actor rewrites the head and then the
//!   envelope as two separately atomic ordered commits, with a valid state between them
//!   (`rewrite_atomically` states the ordering and what a crash there leaves), and issues a
//!   receipt *after* the second commit confirms. The receipt
//!   claims only that the code was updated with the durable contract unchanged — never that
//!   program meaning is preserved. The accepted ceiling is preserved verbatim across the
//!   rebind (a standing maximum is expanded only by conscious re-acceptance).
//! - **Contract changed** — a binding fact differs (an evolution of the durable contract or
//!   the interface). This is a typed refusal, *not* corruption: the store is intact and the
//!   prior program remains usable. It names the changed fact category and points at `marrow
//!   apply`, which owns the typed change review (F03a) that names the exact changed source
//!   places; F02a names the category. The classification runs after the engine's physical
//!   open, so it is what a *changed contract over a healthy engine* yields; an engine that
//!   fails to open surfaces as its own open error instead. Either way the store is not
//!   served.
//!
//! A served store is a [`NativeAttachment`]: the admitted image and the open store behind
//! private fields, so the VM executes only that image against that store. The trusted bulk
//! importer shares the admission gate through [`ImageAdmission`] but requires the exact
//! active binding and never rebinds. An open store holds the store's owner lock, which is
//! non-`Clone` and non-serializable, so no session, bytecode, or client path can enter or
//! forge a lifecycle state, and there is no serialized form to reconstruct one from.

use std::path::Path;

use marrow_codes::Code;
use marrow_image::CeilingDescriptor;
use marrow_kernel::durable::StoreProjection;
use marrow_verify::VerifiedImage;

use crate::attachment::{Attachment, NativeAttachment, PreparedImage};
use crate::authority::{self, DemandExceedsCeiling};
use crate::head::{ActiveBinding, LogicalHead};
use crate::image::{DerivedPin, HeadMapPinMismatch, active_binding, derive_head_map_pin};
use crate::provision::{AdmitError, OpenError, open_admitted};
use crate::store_dir;

/// The ways the admission gate can decline before any engine call: the presented image
/// demands authority beyond the accepted ceiling, the persisted ceiling payload is itself
/// corrupt, or the persisted head-map pin disagrees with the derived numbering. Each maps to
/// a distinct typed refusal at the attach and import entries.
pub(crate) enum AdmissionRefusal {
    /// The image's demand exceeds the accepted ceiling — a typed authority refusal.
    Exceeds(DemandExceedsCeiling),
    /// The persisted accepted-ceiling payload did not decode — store corruption.
    CeilingCorrupt,
    /// The persisted head-map pin disagrees with the numbering this toolchain derives —
    /// fail-closed, the store is never attached under a disagreeing numbering.
    Pin(HeadMapPinMismatch),
}

impl AdmissionRefusal {
    /// The open error a corrupt persisted ceiling payload reports.
    pub(crate) fn ceiling_corrupt() -> OpenError {
        OpenError::Corruption {
            message: "the persisted accepted authority ceiling did not decode".to_string(),
        }
    }
}

/// Why the exact-binding gate the importer runs declined: the head binds another image
/// (a stale presented image or a changed contract), the head names this image with facts the
/// image does not have, or the shared admission gate refused.
pub(crate) enum ExactRefusal {
    /// The head binds a different image whose binding facts are equal: the presented image is
    /// a code-only edit the store has not been rebound to.
    NotActive,
    /// The head names this image's identity but records binding facts the image does not
    /// have — inconsistent binding metadata, recovery-shaped.
    InconsistentBinding,
    /// The head binds a different image with different binding facts.
    ContractChanged(ContractChanged),
    /// The ceiling or pin gate refused.
    Admission(AdmissionRefusal),
}

/// The admission facts one image carries into a locked store: its active binding and the
/// head-map pin this toolchain would serve it under. Derived once, pure over the image and
/// its projection, before the store is touched.
pub(crate) struct ImageAdmission<'a> {
    image: &'a VerifiedImage,
    incoming: ActiveBinding,
    expected_pin: Result<DerivedPin, HeadMapPinMismatch>,
}

impl<'a> ImageAdmission<'a> {
    pub(crate) fn derive(image: &'a VerifiedImage, projection: &StoreProjection) -> Self {
        Self {
            image,
            incoming: active_binding(image),
            expected_pin: derive_head_map_pin(image, projection),
        }
    }

    /// The presented image's active binding.
    pub(crate) fn incoming(&self) -> &ActiveBinding {
        &self.incoming
    }

    /// The attach gate: the accepted ceiling admits the image's whole-program demand, and
    /// when the incoming durable contract is the store's active contract the persisted pin
    /// is exactly the derived binding. A *changed* durable contract is a different graph
    /// whose numbering legitimately differs; that path is classified as the typed
    /// contract-changed refusal after the engine's physical open and never serves the store.
    pub(crate) fn admit_compatible(&self, head: &LogicalHead) -> Result<(), AdmissionRefusal> {
        self.admit_ceiling(head)?;
        if self.incoming.durable_contract == head.binding.durable_contract {
            self.verify_pin(head)?;
        }
        Ok(())
    }

    /// The import gate: the head binds exactly this image, the accepted ceiling admits it,
    /// and the persisted pin is exactly the derived binding — all before the engine opens.
    pub(crate) fn admit_exact(&self, head: &LogicalHead) -> Result<(), ExactRefusal> {
        let stored = &head.binding;
        if self.incoming != *stored {
            return Err(if self.incoming.image_id == stored.image_id {
                ExactRefusal::InconsistentBinding
            } else if stored.facts_equal(&self.incoming) {
                ExactRefusal::NotActive
            } else {
                ExactRefusal::ContractChanged(ContractChanged {
                    changed: classify_delta(stored, &self.incoming),
                })
            });
        }
        self.admit_ceiling(head).map_err(ExactRefusal::Admission)?;
        self.verify_pin(head).map_err(ExactRefusal::Admission)
    }

    /// Reconstruct the accepted ceiling from the persisted head and intersect it with the
    /// presented image's whole-program demand (see `authority::admit`). A ceiling payload
    /// that does not decode is store corruption, not a demand refusal.
    fn admit_ceiling(&self, head: &LogicalHead) -> Result<(), AdmissionRefusal> {
        let accepted = CeilingDescriptor::from_payload(&head.accepted_ceiling)
            .map_err(|_| AdmissionRefusal::CeilingCorrupt)?;
        authority::admit(self.image, &accepted).map_err(AdmissionRefusal::Exceeds)
    }

    /// The head-map pin (FR01 §3): the persisted ledger-id ↔ cell-number bijection must be
    /// exactly the binding the derived pin carries — a disagreement (a drifted numbering, a
    /// permuted or foreign head) would readdress durable cells.
    fn verify_pin(&self, head: &LogicalHead) -> Result<(), AdmissionRefusal> {
        match &self.expected_pin {
            Ok(pin) => pin.verify(&head.head_map).map_err(AdmissionRefusal::Pin),
            Err(mismatch) => Err(AdmissionRefusal::Pin(mismatch.clone())),
        }
    }
}

/// The result of a successful attach: the admitted image paired with the open store.
pub enum AttachOutcome {
    /// The presented image is already the active binding: the head and envelope are
    /// byte-unchanged (taking the lock rewrote its owner marker first, as on every attach).
    /// The store is open and ready.
    AlreadyActive(NativeAttachment),
    /// The image was a binding-only code update: the head and then the envelope were
    /// rewritten to the new image, each commit atomic, and the rebind is committed. The
    /// receipt reports what that commit made active.
    Rebound {
        attachment: NativeAttachment,
        receipt: RebindReceipt,
    },
}

/// What a binding-only rebind reports: the store instance and the newly active image
/// identity, returned only once both commits are durable. Reading one from an
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

/// Which binding fact differs — the category a contract-changed refusal names. The exact
/// changed source places are `marrow apply`'s typed change review (F03a); F02a names the
/// category so the developer knows which kind of change to review. Authority is not a binding
/// fact: a demand change that exceeds the accepted ceiling is the distinct, more actionable
/// [`DemandExceedsCeiling`] refusal, and a demand change within it is admitted.
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

/// A binding-fact delta that is not a binding-only code update: a typed lifecycle refusal,
/// never corruption. The store is intact; the prior program remains usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContractChanged {
    pub changed: ChangedFact,
}

impl ContractChanged {
    /// The stable dotted code — `store.contract_changed`, a typed lifecycle refusal, never
    /// `store.corruption`.
    pub fn code(&self) -> &'static str {
        Code::StoreContractChanged.as_str()
    }
}

impl std::fmt::Display for ContractChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the program image changes {} versus the store's active binding, so it is not a \
             binding-only code update; the store is intact and the prior program remains usable. \
             Run `marrow apply` to review and accept the change",
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
    /// The store's persisted head-map pin (the ledger-id ↔ cell-number bijection, FR01 §3)
    /// disagrees with the (ledger id → cell number) binding this toolchain would serve the
    /// store under. Fail-closed and recovery-shaped: serving the store would readdress
    /// durable cells, so the attach refuses with zero engine calls; head, envelope, and
    /// engine data are unchanged, and only the lock's owner marker was rewritten by
    /// acquisition.
    HeadMapPin(HeadMapPinMismatch),
    /// Rewriting the envelope or head during a rebind failed.
    Io(std::io::Error),
}

impl LifecycleError {
    /// The stable dotted code a tool reports.
    pub fn code(&self) -> &'static str {
        match self {
            LifecycleError::NotExecutable => Code::CliDurableUnsupported.as_str(),
            LifecycleError::Open(error) => error.code(),
            LifecycleError::DemandExceedsCeiling(refusal) => refusal.code(),
            LifecycleError::ContractChanged(refusal) => refusal.code(),
            LifecycleError::HeadMapPin(refusal) => refusal.code(),
            LifecycleError::Io(_) => Code::StoreIo.as_str(),
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
            LifecycleError::Io(error) => write!(f, "the rebind could not be committed: {error}"),
        }
    }
}

impl std::error::Error for LifecycleError {}

/// Attach the prepared image to the store at `dir`, opening it under the image's own store
/// projection. Takes the store's single-owner lock, rereads the persisted head, and
/// classifies the image against the active binding (see the module documentation): an
/// identical image opens already-active, and a binding-only code update is rebound and
/// receipted once both commits confirm. The classification runs after the admission gate
/// and after the engine's physical open, so a binding-fact change is the typed
/// [`LifecycleError::ContractChanged`] refusal pointing at `marrow apply` when the store
/// admits the image and the engine opens; a demand beyond the accepted ceiling, a head-map
/// pin disagreement, or an engine that fails to open surfaces as its own refusal instead. The
/// store is served under none of them. An image with no executable durable shape is refused
/// before the store is touched.
pub fn attach(dir: &Path, prepared: PreparedImage) -> Result<AttachOutcome, LifecycleError> {
    let (image, projection) = prepared.into_parts();
    let Some(projection) = projection else {
        return Err(LifecycleError::NotExecutable);
    };

    // The admission facts are pure over (image, projection); derived here, before the store
    // is touched, so the gate below needs no borrow of the projection the open consumes. The
    // gate runs after the single-owner lock and before any engine call, so a refusal makes
    // zero engine calls.
    let admission = ImageAdmission::derive(&image, &projection);
    let mut opened = match open_admitted(dir, projection, |head| admission.admit_compatible(head)) {
        Ok(opened) => opened,
        Err(AdmitError::Open(error)) => return Err(LifecycleError::Open(error)),
        Err(AdmitError::Refused(AdmissionRefusal::Exceeds(refusal))) => {
            return Err(LifecycleError::DemandExceedsCeiling(refusal));
        }
        Err(AdmitError::Refused(AdmissionRefusal::CeilingCorrupt)) => {
            return Err(LifecycleError::Open(AdmissionRefusal::ceiling_corrupt()));
        }
        Err(AdmitError::Refused(AdmissionRefusal::Pin(refusal))) => {
            return Err(LifecycleError::HeadMapPin(refusal));
        }
    };

    let incoming = *admission.incoming();
    let stored = opened.head.binding;

    // Byte-identical binding: already active, with no head or envelope write.
    if incoming == stored {
        return Ok(AttachOutcome::AlreadyActive(Attachment::new(image, opened)));
    }

    // A binding-fact change is a typed refusal, never corruption.
    if !stored.facts_equal(&incoming) {
        return Err(LifecycleError::ContractChanged(ContractChanged {
            changed: classify_delta(&stored, &incoming),
        }));
    }

    // Binding-only rebind: the durable contract, interface, and ceiling are unchanged and
    // only the image code differs. Atomically commit the head (the active-binding commit
    // point) then the envelope (writer provenance), preserving the head map and reserved slots.
    let new_envelope = crate::envelope::StoreEnvelope {
        writer_toolchain: current_toolchain(),
        ..opened.envelope.clone()
    };
    let new_head = LogicalHead {
        binding: incoming,
        ..opened.head.clone()
    };
    rewrite_atomically(dir, &new_envelope, &new_head).map_err(LifecycleError::Io)?;

    let receipt = RebindReceipt {
        instance: new_envelope.instance,
        new_image_id: incoming.image_id,
    };
    opened.envelope = new_envelope;
    opened.head = new_head;
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
/// writer tuple (FR01 R2).
fn current_toolchain() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Rewrite the head and envelope durably. The head is the active-binding commit point, so it
/// is committed *first* — written to a sibling temporary path, flushed, atomically renamed
/// over the live head, then the directory is flushed so the rename is durable. Only then is
/// the envelope (writer provenance) rewritten the same way and the directory flushed again.
/// Each single-file rename is atomic (a reader sees the file wholly old or wholly new, never
/// torn), and committing the head before the envelope means the recorded provenance can never
/// precede the active binding it describes: a crash between the two leaves the new binding
/// active with slightly stale provenance — forensic-only — never a provenance describing a
/// write the binding does not reflect. The receipt issues only after the final directory
/// flush returns.
fn rewrite_atomically(
    dir: &Path,
    envelope: &crate::envelope::StoreEnvelope,
    head: &LogicalHead,
) -> std::io::Result<()> {
    use crate::durable_fs::{replace_file, sync_dir};
    replace_file(&store_dir::head_path(dir), &head.encode())?;
    sync_dir(dir)?;
    replace_file(&store_dir::envelope_path(dir), &envelope.encode())?;
    sync_dir(dir)
}
