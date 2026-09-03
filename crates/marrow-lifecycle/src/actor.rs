//! The one-shot lifecycle actor: the only path that can change which verified program image
//! a persistent store is bound to, and the only one that compares the store's head-map pin.
//! It is not the only path that pairs an image with a store; the end of this list says which
//! other ones exist and what they skip.
//!
//! An attach takes the store's single-owner lock, rereads the persisted head from disk (never
//! a cached copy), and classifies the incoming image against the store's active binding:
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
//!   already rewritten the lock's owner marker, so the next successful open audits). The
//!   pin guards this attach path; `crate::open` runs no pin comparison and its ordinarily
//!   spelled production callers are censused by the lifecycle test battery, which lists the
//!   spellings — a root alias among them — that it does not reach.
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
//! `attach` is the only entry that pairs a store with a verified image *and compares the
//! pin*, and the only one that can rebind. It is not the only way an image and a store meet:
//! `crate::open` returns an [`OpenStore`] with no image and no pin comparison, and a caller
//! holding one can pair any verified image with it through the runner's public
//! `AttachedService::new`, which executes that image against that store. Closing that
//! composition belongs to the follow-on row, together with threading an image through
//! `import_jsonl`. Here, the lifecycle test battery censuses the production callers of the
//! `open` *spelling* as it is ordinarily written, so a new ASCII call spelled bare or under
//! one of the qualifiers the census resolves turns up. It does not reach a submodule re-export
//! called under another path, a public wrapper over `open_admitted`, a dependency rename
//! inherited from the workspace manifest, a call a macro emits, a root alias
//! (`use crate as life`), a call separated by non-ASCII whitespace, or a binding whose name
//! uses a decomposed accent — each of which can still produce an unpinned `OpenStore`
//! unseen, and each of which is listed at the census itself.
//! An unfenced pairing is therefore fenced against the ordinary direct route only, and this
//! census is a stand-in until the follow-on row makes `open` unavailable outside a fenced
//! entry, at which point it retires rather than hardens. An `OpenStore` holds the store's
//! owner lock, which is non-`Clone` and non-serializable, so no session, bytecode, or client
//! path can enter or forge a lifecycle state — nothing below this crate depends on it (the
//! Cargo trust boundary), and there is no serialized form to reconstruct one from.

use std::path::Path;

use marrow_codes::Code;
use marrow_image::CeilingDescriptor;
use marrow_kernel::durable::StoreProjection;
use marrow_verify::VerifiedImage;

use crate::authority::{self, DemandExceedsCeiling};
use crate::head::{ActiveBinding, LogicalHead};
use crate::image::{HeadMapPinMismatch, active_binding, derive_head_map_pin};
use crate::provision::{AdmitError, OpenError, OpenStore, open_admitted};
use crate::store_dir;

/// The ways the attach admission gate can decline before any engine call: the presented
/// image demands authority beyond the accepted ceiling, the persisted ceiling payload is
/// itself corrupt, or the persisted head-map pin disagrees with the derived numbering. Kept
/// private to the actor; each maps to a distinct [`LifecycleError`].
enum AdmissionRefusal {
    /// The image's demand exceeds the accepted ceiling — a typed authority refusal.
    Exceeds(DemandExceedsCeiling),
    /// The persisted accepted-ceiling payload did not decode — store corruption.
    CeilingCorrupt,
    /// The persisted head-map pin disagrees with the numbering this toolchain derives —
    /// fail-closed, the store is never attached under a disagreeing numbering.
    Pin(HeadMapPinMismatch),
}

/// The result of a successful attach.
pub enum AttachOutcome {
    /// The presented image is already the active binding: the head and envelope are
    /// byte-unchanged (taking the lock rewrote its owner marker first, as on every attach).
    /// The store is open and ready.
    AlreadyActive(OpenStore),
    /// The image was a binding-only code update: the head and then the envelope were
    /// rewritten to the new image, each commit atomic, and the rebind is committed. The
    /// receipt reports what that commit made active.
    Rebound {
        store: OpenStore,
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
            LifecycleError::Open(error) => write!(f, "{error}"),
            LifecycleError::DemandExceedsCeiling(refusal) => write!(f, "{refusal}"),
            LifecycleError::ContractChanged(refusal) => write!(f, "{refusal}"),
            LifecycleError::HeadMapPin(refusal) => write!(f, "{refusal}"),
            LifecycleError::Io(error) => write!(f, "the rebind could not be committed: {error}"),
        }
    }
}

impl std::error::Error for LifecycleError {}

/// Attach the verified `image` to the store at `dir`, opening it under the store shape
/// `projection` describes. Takes the store's single-owner lock, rereads the persisted
/// head, and classifies the image against the active binding (see the module documentation):
/// an identical image opens already-active, and a binding-only code update is rebound and
/// receipted once both commits confirm. The classification runs after the admission gate
/// and after the engine's physical open, so a binding-fact change is the typed
/// [`LifecycleError::ContractChanged`] refusal pointing at `marrow apply` when the store
/// admits the image and the engine opens; a demand beyond the accepted ceiling, a head-map
/// pin disagreement, or an engine that fails to open surfaces as its own refusal instead. The
/// store is served under none of them.
pub fn attach(
    dir: &Path,
    image: &VerifiedImage,
    projection: StoreProjection,
) -> Result<AttachOutcome, LifecycleError> {
    let incoming = active_binding(image);

    // The (ledger id → cell number) binding this toolchain would actually serve the store
    // under: the kernel's numbering of this exact projection, paired to the image's durable
    // identities by source name and node kind (`crate::image::derive_head_map_pin`). Pure over
    // (image, projection); derived here, before the store is touched, so the admission
    // closure below needs no borrow of the projection the open consumes.
    let expected_pin = derive_head_map_pin(image, &projection);

    // The admission gate runs after the single-owner lock and before any engine call, so an
    // image whose demand exceeds the store's accepted ceiling is refused with zero engine
    // calls. The gate reconstructs the accepted ceiling from the persisted head and intersects
    // it with the presented image's whole-program demand (see `authority::admit`). A ceiling
    // payload that does not decode is store corruption, not a demand refusal.
    let admit = |head: &LogicalHead| -> Result<(), AdmissionRefusal> {
        let accepted = CeilingDescriptor::from_payload(&head.accepted_ceiling)
            .map_err(|_| AdmissionRefusal::CeilingCorrupt)?;
        authority::admit(image, &accepted).map_err(AdmissionRefusal::Exceeds)?;
        // The head-map pin (FR01 §3): when the incoming durable contract is the store's
        // active contract, the persisted ledger-id ↔ cell-number bijection must be exactly
        // the binding the derived pin carries — a disagreement (a drifted numbering, a
        // permuted or foreign head) would readdress durable cells, so the store is refused
        // here, with zero engine calls, before anything can read or write under the wrong
        // numbers. A *changed* durable contract is a different graph whose numbering
        // legitimately differs; that path is classified as the typed contract-changed
        // refusal after the engine's physical open (and, after an unclean shutdown, its
        // integrity audit — numbering-independent physical access), before any session, and
        // never serves the store. A failing engine on that path surfaces as its own open
        // error rather than the contract refusal.
        if incoming.durable_contract == head.binding.durable_contract {
            match &expected_pin {
                Ok(pin) => pin.verify(&head.head_map).map_err(AdmissionRefusal::Pin)?,
                Err(mismatch) => return Err(AdmissionRefusal::Pin(mismatch.clone())),
            }
        }
        Ok(())
    };
    let mut opened = match open_admitted(dir, projection, admit) {
        Ok(opened) => opened,
        Err(AdmitError::Open(error)) => return Err(LifecycleError::Open(error)),
        Err(AdmitError::Refused(AdmissionRefusal::Exceeds(refusal))) => {
            return Err(LifecycleError::DemandExceedsCeiling(refusal));
        }
        Err(AdmitError::Refused(AdmissionRefusal::CeilingCorrupt)) => {
            return Err(LifecycleError::Open(OpenError::Corruption {
                message: "the persisted accepted authority ceiling did not decode".to_string(),
            }));
        }
        Err(AdmitError::Refused(AdmissionRefusal::Pin(refusal))) => {
            return Err(LifecycleError::HeadMapPin(refusal));
        }
    };

    let stored = opened.head.binding;

    // Byte-identical binding: already active, with no head or envelope write.
    if incoming == stored {
        return Ok(AttachOutcome::AlreadyActive(opened));
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
        store: opened,
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
