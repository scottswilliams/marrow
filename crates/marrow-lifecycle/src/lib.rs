//! Privileged persistent-store lifecycle.
//!
//! `marrow-lifecycle` owns the persistent store's identity and durability contracts and,
//! as the F-chain builds out, the privileged provision/open composition over the image,
//! runtime, and engine owners — so `marrow-runtime`/`marrow-vm` stay session-free and no
//! application path can enter a lifecycle state. It is the single owner of:
//!
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
mod reopen;
mod report;
mod slice;
mod store_dir;

pub use actor::{
    AttachOutcome, ChangedFact, ContractChanged, LifecycleError, RebindReceipt, attach,
};
pub use codec::FormatError;
pub use envelope::{EngineKind, StoreEnvelope};
pub use head::{ActiveBinding, LogicalHead};
pub use headmap::{HeadMap, HeadMapEntry, MAX_HEAD_MAP_ENTRIES};
pub use image::{active_binding, head_map, head_map_node_order};
pub use import::{
    CommitFault, ImportError, ImportLimits, ImportReport, ImportTarget, RowFault, ShapeFault,
    import_jsonl,
};
pub use instance::{EntropyUnavailable, StoreInstanceId};
pub use lock::{Acquired, LockError, LockOwner, OwnerLock};
pub use provision::{
    OpenError, OpenStore, Preflight, ProvisionError, ProvisionRequest, Provisioned, open,
    preflight, provision,
};
pub use reopen::reopen_and_classify;
pub use report::{ProvisionApproval, ProvisionImageError, ProvisionReport, provision_image};
pub use slice::{SliceError, backup_slice, restore_slice};
pub use store_dir::{ENGINE_FILE, ENVELOPE_FILE, HEAD_FILE, LOCK_FILE};
