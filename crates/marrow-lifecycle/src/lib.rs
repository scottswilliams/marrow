//! Privileged persistent-store lifecycle.
//!
//! `marrow-lifecycle` owns the persistent store's identity and durability contracts and the
//! privileged provision/attach/import composition over the image, runtime, and engine
//! owners. It is the single owner of:
//!
//! - the image/store attachment ([`PreparedImage`], [`Attachment`], [`FreshTest`]): an
//!   image's store projection is derived once, and only this crate pairs an image with the
//!   native or in-memory host it admitted for that image — the VM executes durable exports
//!   through that pairing alone, so no application path can enter a lifecycle state or pair
//!   an image with a store it was not admitted for;
//! - the store's own identity ([`StoreInstanceId`], entropy-minted at provision);
//! - the persisted [`StoreEnvelope`] recording store instance and writer/engine provenance;
//! - the logical active [`LogicalHead`] recording the active binding, the FR01 reserved
//!   sequencing and data-digest slots, and the head identity map;
//! - the head identity map ([`HeadMap`]), the store-local ledger-id ↔ number bijection the
//!   id-keyed cell layout is prefixed by.
//!
//! Every persisted artifact is a versioned, big-endian, length-prefixed container sealed by
//! a domain-separated digest, decoded strictly (unknown version, over-bound length, unknown
//! discriminant, digest mismatch, and trailing bytes all reject) through the shared
//! [`codec`] reader. The digest kinds and framing live in `marrow-image`, the workspace's
//! identity-framing owner, so this crate composes them without a hash dependency of its own.

mod actor;
mod attachment;
mod authority;
mod codec;
mod durable_fs;
mod envelope;
mod head;
mod headmap;
mod image;
mod import;
mod instance;
mod lock;
mod provision;
mod report;
mod store_dir;

#[cfg(test)]
mod owner_first_admission_tests;
#[cfg(test)]
mod provision_lifecycle_tests;

pub use actor::{
    AttachOutcome, ChangedFact, ContractChanged, LifecycleError, RebindReceipt, attach,
};
pub use attachment::{
    Attachment, EphemeralOutcome, FreshTest, MemoryAttachment, MemoryEngine, NativeAttachment,
    PreparedImage, TestExecution, TestHost, fresh_test, mint_ephemeral, prepare,
};
pub use authority::{DemandExceedsCeiling, ExceedingDemand};
pub use codec::FormatError;
pub use envelope::{EngineKind, MAX_ENVELOPE_FILE_BYTES, StoreEnvelope};
pub use head::{ActiveBinding, LogicalHead, MAX_HEAD_FILE_BYTES};
pub use headmap::{HeadMap, HeadMapEntry, MAX_HEAD_MAP_ENTRIES};
pub use image::{
    HeadMapPinMismatch, PinDisagreement, accepted_ceiling, active_binding, head_map,
    head_map_node_order, verify_head_map_pin,
};
pub use import::{
    CommitFault, ImportError, ImportLimits, ImportReport, ImportTarget, RowFault, ShapeFault,
    import_jsonl,
};
pub use instance::{EntropyUnavailable, StoreInstanceId};
pub use lock::{LockError, LockOwner};
// The invocation grant `import_jsonl` requires. It is a kernel type, re-exported here so a
// privileged caller (the companion runner's `import` command) can name and mint the full-store
// grant a trusted bulk import runs under without depending on the kernel directly.
pub use marrow_kernel::durable::InvocationGrant;
// The typed custody refusal an [`AdmissionFault::Custody`] carries. It belongs to
// `marrow-fs-journal`, the workspace's sole owner of descriptor-rooted filesystem operations,
// and is re-exported here so a caller matching that variant can name its payload without
// taking an edge to the adapter crate itself.
pub use marrow_fs_journal::CustodyError;
pub use provision::{
    OpenError, OpenStore, Preflight, ProvisionError, ProvisionRequest, Provisioned, preflight,
    provision,
};
pub use report::{ProvisionApproval, ProvisionImageError, ProvisionReport, provision_image};
pub use store_dir::{
    AdmissionError, AdmissionFault, ENGINE_FILE, ENVELOPE_FILE, HEAD_FILE, Instability, LOCK_FILE,
    StoreAccessError, StoreEntry,
};
