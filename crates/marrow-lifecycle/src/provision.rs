//! The persistent provision and open flow.
//!
//! Provision publishes a store *complete-or-not-at-all*: it builds the whole store in a
//! private sibling temporary directory and atomically renames it into place. A rename onto
//! an existing non-empty store directory fails, so exactly one provisioner wins a race and a
//! crash before the rename leaves only a temporary directory — the destination is never a
//! partially-formed store (the publication-uncertainty boundary). Preflight is strictly
//! non-creating, so probing a destination never leaves a file behind.
//!
//! Open requires a complete store, takes the single-owner lock (naming the live owner on
//! contention), decodes the envelope and head, and opens the engine through the path kernel.
//! When the prior shutdown was unclean (a stale owner descriptor in the lock) it runs a full
//! integrity audit.
//!
//! **Coverage honesty.** The unclean-open audit covers crash-path corruption only: the fast
//! open path does not re-verify page checksums, so an externally flipped bit in a
//! cleanly-closed store stays undetected here until the FR01 §2 data-root digest is populated
//! by a later full-walk operation (audit/backup/restore at F04+). No mitigation is claimed
//! for that class at open.

use std::path::{Path, PathBuf};

use marrow_codes::Code;
use marrow_kernel::durable::{
    CommitRecovery, DemandCoverage, DurableCommitState, InvocationGrant, NativeOwnerOpenError,
    NativeStore, ReadSession, SessionError, SessionHost, SiteSpec, StoreError, StoreSchema,
    TxnSession,
};

use crate::codec::FormatError;
use crate::durable_fs::{sync_dir, write_file};
use crate::envelope::StoreEnvelope;
use crate::head::LogicalHead;
use crate::instance::StoreInstanceId;
use crate::lock::LockError;
use crate::store_dir;

/// A non-creating classification of a store directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preflight {
    /// No store directory exists at the path.
    Absent,
    /// The directory exists but is missing at least one durable artifact — a partially
    /// formed store, never published as complete (a leftover of an interrupted build).
    Incomplete,
    /// The directory exists with all durable artifacts present.
    Complete,
}

/// Classify the store at `dir` without creating or modifying anything. Reads only: it stats
/// the directory and its artifacts. A missing directory is [`Preflight::Absent`]; a directory
/// missing any of the engine, envelope, or head is [`Preflight::Incomplete`]; a directory
/// with all three is [`Preflight::Complete`].
pub fn preflight(dir: &Path) -> Preflight {
    if !dir.is_dir() {
        return Preflight::Absent;
    }
    if store_dir::artifacts_present(dir) {
        Preflight::Complete
    } else {
        Preflight::Incomplete
    }
}

/// The inputs to a provision: the persisted envelope and logical head to publish, and the
/// schema and site tables the engine is created under. The caller (the lifecycle actor)
/// derives these from a verified image; F02a provisions an empty engine (no user data).
pub struct ProvisionRequest {
    pub envelope: StoreEnvelope,
    pub head: LogicalHead,
    pub schemas: Vec<StoreSchema>,
    pub sites: Vec<SiteSpec>,
}

/// The outcome of a successful provision: the store instance now published at the
/// destination. The store is left closed; the caller opens it to drive sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provisioned {
    pub instance: StoreInstanceId,
}

/// Why a provision failed.
#[derive(Debug)]
pub enum ProvisionError {
    /// A complete or partially-formed store already occupies the destination: the caller lost
    /// the one-winner claim, or a prior provision is present. The destination is untouched.
    AlreadyProvisioned,
    /// The ordered-byte engine could not be created.
    Store(StoreError),
    /// A filesystem operation failed.
    Io(std::io::Error),
}

impl ProvisionError {
    /// The stable dotted code a tool reports.
    pub fn code(&self) -> &'static str {
        match self {
            ProvisionError::AlreadyProvisioned => Code::StoreLocked.as_str(),
            ProvisionError::Store(error) => error.code(),
            ProvisionError::Io(_) => Code::StoreIo.as_str(),
        }
    }
}

impl std::fmt::Display for ProvisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProvisionError::AlreadyProvisioned => {
                write!(f, "a store already exists at the destination")
            }
            ProvisionError::Store(error) => {
                write!(f, "the store engine could not be created: {error}")
            }
            ProvisionError::Io(error) => write!(f, "provisioning failed: {error}"),
        }
    }
}

impl std::error::Error for ProvisionError {}

/// Provision a fresh store at `dest`, publishing it complete-or-not-at-all. Builds the whole
/// store in a private sibling temporary directory (owner-only, mode `0700`) — the engine
/// database created through the path kernel, then the envelope and head bytes written and
/// flushed — then atomically renames it onto `dest`. A rename onto an existing non-empty
/// destination fails, so exactly one racing provisioner wins and the destination is never
/// left partial. On any failure before the rename, the temporary directory is removed, so a
/// failed provision leaves no published file.
pub fn provision(dest: &Path, request: ProvisionRequest) -> Result<Provisioned, ProvisionError> {
    let instance = request.envelope.instance;
    let temp = temp_sibling(dest);
    // Build the whole store in the temp directory; on any error, remove it and surface.
    match build_in_temp(&temp, &request) {
        Ok(()) => {}
        Err(error) => {
            let _ = std::fs::remove_dir_all(&temp);
            return Err(error);
        }
    }

    // The one-winner atomic claim: rename the fully-formed temp directory onto the
    // destination. A rename onto an existing non-empty directory fails (the destination is
    // an existing store or another winner's claim), so the loser cleans up its temp and
    // reports the destination taken.
    match std::fs::rename(&temp, dest) {
        Ok(()) => {}
        Err(error) => {
            let _ = std::fs::remove_dir_all(&temp);
            // A destination that now exists is another winner (or a prior store), not our
            // I/O fault.
            return if dest.exists() {
                Err(ProvisionError::AlreadyProvisioned)
            } else {
                Err(ProvisionError::Io(error))
            };
        }
    }
    // Make the new directory entry durable in the parent.
    if let Some(parent) = dest.parent() {
        sync_dir(parent).map_err(ProvisionError::Io)?;
    }
    Ok(Provisioned { instance })
}

/// Build the store's artifacts in the private temporary directory `temp`: create the
/// owner-only directory, create the engine database through the path kernel, write the
/// envelope and head bytes, and flush every file and the directory to disk.
fn build_in_temp(temp: &Path, request: &ProvisionRequest) -> Result<(), ProvisionError> {
    create_private_dir(temp).map_err(ProvisionError::Io)?;

    // Provisioning is the sole create/stamp path. It returns no engine or store
    // capability, so the newly created body cannot escape without an owner lock.
    NativeStore::provision(temp).map_err(ProvisionError::Store)?;

    write_file(&store_dir::envelope_path(temp), &request.envelope.encode())
        .map_err(ProvisionError::Io)?;
    write_file(&store_dir::head_path(temp), &request.head.encode()).map_err(ProvisionError::Io)?;
    sync_dir(temp).map_err(ProvisionError::Io)?;
    Ok(())
}

/// A held-open provisioned store: the native store the kernel drives, its envelope and head,
/// and the single-owner lock. An ordinary close releases the lock; an unresolved commit
/// quarantines it until process exit. The engine and lock are private and inseparable; callers
/// receive only this session-host capability.
///
/// ```compile_fail
/// use marrow_kernel::durable::NativeStore;
/// use marrow_lifecycle::OpenStore;
/// fn detach_engine(mut opened: OpenStore) {
///     let _: &mut NativeStore = &mut opened.store;
///     let _lock = opened.lock;
/// }
/// ```
pub struct OpenStore {
    owner: NativeStore,
    pub envelope: StoreEnvelope,
    pub head: LogicalHead,
}

impl SessionHost for OpenStore {
    type Engine = <NativeStore as SessionHost>::Engine;

    fn read_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<ReadSession<'_, Self::Engine>, SessionError> {
        self.owner.read_session(grant, demand)
    }

    fn txn_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<TxnSession<'_, Self::Engine>, SessionError> {
        self.owner.txn_session(grant, demand)
    }
}

impl OpenStore {
    /// Consume an indeterminate commit's sole affine fact while retaining the
    /// same owner lock across old-engine close, fresh reopen, full integrity
    /// audit, and exact witness comparison. A known result returns the freshly
    /// opened owner for later invocations in this process, but quarantine stays
    /// irreversible and its drop cannot release the lock. Unknown retires the
    /// owner under the same process-lifetime quarantine.
    pub fn resolve_recovery(self, recovery: CommitRecovery) -> (DurableCommitState, Option<Self>) {
        let Self {
            owner,
            envelope,
            head,
        } = self;
        let (state, owner) = owner.resolve_recovery(recovery);
        (
            state,
            owner.map(|owner| Self {
                owner,
                envelope,
                head,
            }),
        )
    }
}

/// Why an open failed.
#[derive(Debug)]
pub enum OpenError {
    /// No store exists at the path.
    NotProvisioned,
    /// The store directory exists but is missing a durable artifact.
    Incomplete,
    /// The store is held by another owner, or the lock could not be taken.
    Lock(LockError),
    /// The persisted envelope or head bytes did not decode. Carries the typed
    /// [`FormatError`] so an unknown writer version (`store.format_version`) or an over-bound
    /// field (`store.limit`) is reported as itself, not flattened to corruption (FR01 §6).
    Decode(FormatError),
    /// The unclean-open integrity audit found the engine's stored bytes corrupt.
    Corruption { message: String },
    /// The ordered-byte engine could not be opened.
    Store(StoreError),
    /// A filesystem operation failed.
    Io(std::io::Error),
}

impl OpenError {
    /// The stable dotted code a tool reports.
    pub fn code(&self) -> &'static str {
        match self {
            OpenError::NotProvisioned => Code::StoreIo.as_str(),
            OpenError::Incomplete | OpenError::Corruption { .. } => Code::StoreCorruption.as_str(),
            OpenError::Decode(error) => error.code(),
            OpenError::Lock(error) => error.code(),
            OpenError::Store(error) => error.code(),
            OpenError::Io(_) => Code::StoreIo.as_str(),
        }
    }
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::NotProvisioned => write!(f, "no store exists at the destination"),
            OpenError::Incomplete => {
                write!(
                    f,
                    "the store directory is incomplete (a partially-formed store)"
                )
            }
            OpenError::Lock(error) => write!(f, "{error}"),
            OpenError::Decode(error) => write!(f, "the store {error}"),
            OpenError::Corruption { message } => write!(f, "the store is corrupt: {message}"),
            OpenError::Store(error) => write!(f, "the store engine could not be opened: {error}"),
            OpenError::Io(error) => write!(f, "opening the store failed: {error}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// A failure to open a store under an admission gate: either the ordinary open failed, or the
/// gate refused the presented image after the lock was taken and before any engine call. The
/// `R` is the caller's refusal type (the lifecycle actor's demand-exceeds-ceiling refusal).
pub(crate) enum AdmitError<R> {
    /// The store could not be opened (see [`OpenError`]).
    Open(OpenError),
    /// The admission gate refused the image with zero engine calls; the lock was released.
    Refused(R),
}

/// Open the complete store at `dir` under `schemas`/`sites`, taking the single-owner lock. A
/// non-complete directory is refused without opening; a store held by another owner returns
/// [`OpenError::Lock`] naming the owner. When the prior shutdown was unclean (a stale lock
/// descriptor) a full integrity audit runs, mapping a failure to corruption. On success the
/// returned [`OpenStore`] holds the lock for the store's whole open life.
pub fn open(
    dir: &Path,
    schemas: Vec<StoreSchema>,
    sites: Vec<SiteSpec>,
) -> Result<OpenStore, OpenError> {
    open_admitted(dir, schemas, sites, |_| {
        Ok::<(), std::convert::Infallible>(())
    })
    .map_err(|error| match error {
        AdmitError::Open(open) => open,
        // The no-op admit never refuses.
        AdmitError::Refused(never) => match never {},
    })
}

/// Open the complete store at `dir`, running `admit` against the persisted head **after** the
/// single-owner lock is taken and **before** any engine call, so a refusal makes zero engine
/// calls and releases the lock on return. The lifecycle actor supplies an `admit` that
/// reconstructs the store's accepted deployment ceiling from the head and intersects it with
/// the presented image's demand; a refusal is surfaced as [`AdmitError::Refused`]. The plain
/// [`open`] passes a no-op admit.
pub(crate) fn open_admitted<R>(
    dir: &Path,
    schemas: Vec<StoreSchema>,
    sites: Vec<SiteSpec>,
    admit: impl FnOnce(&LogicalHead) -> Result<(), R>,
) -> Result<OpenStore, AdmitError<R>> {
    match preflight(dir) {
        Preflight::Absent => return Err(AdmitError::Open(OpenError::NotProvisioned)),
        Preflight::Incomplete => return Err(AdmitError::Open(OpenError::Incomplete)),
        Preflight::Complete => {}
    }

    // Pin the complete store directory before any callback or engine open. Retaining caller-
    // relative text would allow a later cwd change to redirect indeterminate-commit recovery
    // while the original store's owner lock remained held.
    let dir = std::fs::canonicalize(dir).map_err(|error| AdmitError::Open(OpenError::Io(error)))?;
    if preflight(&dir) != Preflight::Complete {
        return Err(AdmitError::Open(OpenError::Incomplete));
    }

    let envelope = decode_envelope(&dir).map_err(AdmitError::Open)?;
    let mut admitted_head = None;

    // The kernel capsule composes lock, admission, existing-only engine open,
    // and any required unclean-open audit without exposing its lower owner.
    let owner = NativeStore::open_existing_admitted(
        &dir,
        *envelope.instance.bytes(),
        schemas,
        sites,
        || {
            // Read and admit the mutable logical head only after the physical
            // owner lock is held. The callback receives no store capability.
            let head = decode_head(&dir).map_err(Ok)?;
            admit(&head).map_err(Err)?;
            admitted_head = Some(head);
            Ok::<(), Result<OpenError, R>>(())
        },
    )
    .map_err(|error| match error {
        NativeOwnerOpenError::Io(error) => AdmitError::Open(OpenError::Io(error)),
        NativeOwnerOpenError::Lock(error) => {
            AdmitError::Open(OpenError::Lock(LockError::from(error)))
        }
        NativeOwnerOpenError::Refused(Ok(error)) => AdmitError::Open(error),
        NativeOwnerOpenError::Refused(Err(refusal)) => AdmitError::Refused(refusal),
        NativeOwnerOpenError::Store(StoreError::Corruption { message }) => {
            AdmitError::Open(OpenError::Corruption { message })
        }
        NativeOwnerOpenError::Store(error) => AdmitError::Open(OpenError::Store(error)),
    })?;

    Ok(OpenStore {
        owner,
        envelope,
        head: admitted_head.expect("a successful open completed head admission"),
    })
}

fn decode_envelope(dir: &Path) -> Result<StoreEnvelope, OpenError> {
    let bytes = std::fs::read(store_dir::envelope_path(dir)).map_err(OpenError::Io)?;
    StoreEnvelope::decode(&bytes).map_err(OpenError::Decode)
}

fn decode_head(dir: &Path) -> Result<LogicalHead, OpenError> {
    let bytes = std::fs::read(store_dir::head_path(dir)).map_err(OpenError::Io)?;
    LogicalHead::decode(&bytes).map_err(OpenError::Decode)
}

/// A private sibling temporary directory for building a store before its atomic claim: the
/// destination's own name prefixed with a recognizable marker plus the process id and a
/// monotonic counter, so concurrent provisioners never collide and a leaked temp (from a
/// crash before the rename) is identifiable and never mistaken for a published store.
pub(crate) fn temp_sibling(dest: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = dest
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "store".to_string());
    let temp_name = format!(".{name}.provisioning.{}.{counter}", std::process::id());
    match dest.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(temp_name),
        _ => PathBuf::from(temp_name),
    }
}

#[cfg(unix)]
pub(crate) fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(dir)
}

#[cfg(not(unix))]
pub(crate) fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new().create(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ScratchDir(std::path::PathBuf);

    impl ScratchDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "marrow-lifecycle-{tag}-{}-{}",
                std::process::id(),
                now_nonce(),
            ));
            std::fs::create_dir_all(&path).expect("create scratch directory");
            Self(path)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn preflight_classifies_absent_incomplete_complete_without_creating() {
        let base = std::env::temp_dir().join(format!(
            "marrow-lifecycle-preflight-{}-{}",
            std::process::id(),
            now_nonce(),
        ));
        let dir = base.join("store");

        // Absent: no directory. Preflight creates nothing.
        assert_eq!(preflight(&dir), Preflight::Absent);
        assert!(
            !base.exists(),
            "preflight must not create the base directory"
        );
        assert!(
            !dir.exists(),
            "preflight must not create the store directory"
        );

        // Incomplete: the directory exists but lacks artifacts.
        std::fs::create_dir_all(&dir).expect("create dir");
        assert_eq!(preflight(&dir), Preflight::Incomplete);
        let before: Vec<_> = read_dir_names(&dir);
        assert_eq!(preflight(&dir), Preflight::Incomplete);
        assert_eq!(
            read_dir_names(&dir),
            before,
            "preflight must not add a file"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn opaque_native_owner_holds_exclusion_and_clean_drop_releases_it() {
        let scratch = ScratchDir::new("opaque-owner");
        NativeStore::provision(&scratch.0).expect("provision native engine");
        let owner = NativeStore::open_existing_admitted(
            &scratch.0,
            [0x51; 16],
            Vec::new(),
            Vec::new(),
            || Ok::<_, std::convert::Infallible>(()),
        )
        .expect("open owner");
        assert!(matches!(
            NativeStore::open_existing_admitted(
                &scratch.0,
                [0x52; 16],
                Vec::new(),
                Vec::new(),
                || Ok::<_, std::convert::Infallible>(()),
            ),
            Err(NativeOwnerOpenError::Lock(_)),
        ));
        drop(owner);
        NativeStore::open_existing_admitted(&scratch.0, [0x52; 16], Vec::new(), Vec::new(), || {
            Ok::<_, std::convert::Infallible>(())
        })
        .expect("clean drop releases exclusion");
    }

    fn read_dir_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn now_nonce() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }
}
