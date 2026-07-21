//! The one-shot lifecycle actor: the single privileged path that binds a verified program
//! image to a persistent store.
//!
//! An attach takes the store's single-owner lock, rereads the persisted head from disk (never
//! a cached copy), and classifies the incoming image against the store's active binding:
//!
//! - **Already active** — the image is byte-identical to the active binding; the store opens
//!   with no write.
//! - **Binding-only rebind** — the durable contract, interface, and ceiling are all unchanged
//!   and only the image's code (its byte identity) differs. The actor atomically rewrites the
//!   envelope and head to the new image and issues a receipt *after* the commit confirms. The
//!   receipt claims only that the code was updated with the durable contract unchanged — never
//!   that program meaning is preserved.
//! - **Contract changed** — any binding fact differs (an evolution of the durable contract,
//!   the interface, or the ceiling). This is a typed refusal, *not* corruption: the store is
//!   intact and the prior program remains usable. It names the changed fact category and
//!   points at `marrow apply`, which owns the typed change review (F03a) that names the exact
//!   changed source places; F02a names the category.
//!
//! The actor is the sole constructor of a lifecycle transition: it returns a live
//! [`OpenStore`] holding the store's owner lock, which is non-`Clone` and non-serializable, so
//! no session, bytecode, or client path can enter or forge a lifecycle state — nothing below
//! this crate depends on it (the Cargo trust boundary), and there is no serialized form to
//! reconstruct one from.

use std::path::Path;

use marrow_codes::Code;
use marrow_kernel::durable::{SiteSpec, StoreSchema};
use marrow_verify::VerifiedImage;

use crate::head::{ActiveBinding, LogicalHead};
use crate::image::active_binding;
use crate::provision::{OpenError, OpenStore, open};
use crate::store_dir;

/// The result of a successful attach.
pub enum AttachOutcome {
    /// The presented image is already the active binding: no write occurred. The store is
    /// open and ready.
    AlreadyActive(OpenStore),
    /// The image was a binding-only code update: the envelope and head were atomically
    /// rewritten to the new image and the rebind is committed. The receipt proves the commit.
    Rebound {
        store: OpenStore,
        receipt: RebindReceipt,
    },
}

/// The confirmed-commit receipt of a binding-only rebind, issued only after the head rewrite
/// has been made durable. It records the store instance and the newly active image identity;
/// its meaning is exactly "the active code was updated, the durable contract unchanged".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RebindReceipt {
    pub instance: crate::instance::StoreInstanceId,
    pub new_image_id: [u8; 32],
}

/// Which binding fact differs — the category a contract-changed refusal names. The exact
/// changed source places are `marrow apply`'s typed change review (F03a); F02a names the
/// category so the developer knows which kind of change to review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangedFact {
    /// The durable contract — the durable graph over ledger ids — changed (an evolution).
    DurableContract,
    /// The exported interface — the call surface — changed.
    Interface,
    /// The authority ceiling over the demand union changed.
    Ceiling,
}

impl ChangedFact {
    fn describe(self) -> &'static str {
        match self {
            ChangedFact::DurableContract => "the durable contract",
            ChangedFact::Interface => "the exported interface",
            ChangedFact::Ceiling => "the authority ceiling",
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
    /// The image is not a binding-only code update — a typed refusal pointing at `marrow
    /// apply`, never corruption.
    ContractChanged(ContractChanged),
    /// Rewriting the envelope or head during a rebind failed.
    Io(std::io::Error),
}

impl LifecycleError {
    /// The stable dotted code a tool reports.
    pub fn code(&self) -> &'static str {
        match self {
            LifecycleError::Open(error) => error.code(),
            LifecycleError::ContractChanged(refusal) => refusal.code(),
            LifecycleError::Io(_) => Code::StoreIo.as_str(),
        }
    }
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LifecycleError::Open(error) => write!(f, "{error}"),
            LifecycleError::ContractChanged(refusal) => write!(f, "{refusal}"),
            LifecycleError::Io(error) => write!(f, "the rebind could not be committed: {error}"),
        }
    }
}

impl std::error::Error for LifecycleError {}

/// Attach the verified `image` to the store at `dir`, opening it under the store shape
/// `schemas`/`sites` describe. Takes the store's single-owner lock, rereads the persisted
/// head, and classifies the image against the active binding (see the module documentation):
/// an identical image opens already-active, a binding-only code update is atomically rebound
/// and receipted, and any binding-fact change is a typed [`LifecycleError::ContractChanged`]
/// refusal pointing at `marrow apply`.
pub fn attach(
    dir: &Path,
    image: &VerifiedImage,
    schemas: Vec<StoreSchema>,
    sites: Vec<SiteSpec>,
) -> Result<AttachOutcome, LifecycleError> {
    let mut opened = open(dir, schemas, sites).map_err(LifecycleError::Open)?;

    let incoming = active_binding(image);
    let stored = opened.head.binding;

    // Byte-identical binding: no write, already active.
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
/// checked in a fixed order (durable contract, interface, ceiling). At least one differs
/// because the caller has established `!facts_equal`.
fn classify_delta(stored: &ActiveBinding, incoming: &ActiveBinding) -> ChangedFact {
    if stored.durable_contract != incoming.durable_contract {
        ChangedFact::DurableContract
    } else if stored.interface != incoming.interface {
        ChangedFact::Interface
    } else {
        ChangedFact::Ceiling
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
